#!/usr/bin/env python3
"""
MOON.lite router aggregator - minimal end-to-end Python sample client.

Flow (fast submit path):
  1) GET  /quote  -> print a human-readable preview
  2) POST /swap   -> get {auth, hops, to, netOut}  (trader = your address, NO privateKey)
  3) EIP-712 sign the returned `auth` (SwapAuthorization) with your local key
  4) ERC20 approve(router, amountIn) on tokenIn if allowance < amountIn (via wallet)
  5) BUILD + locally SIGN the swapExactIn transaction and POST it to /submit
     for faster inclusion. Print the returned txHash.

The API is public: /quote and /swap need no auth. The server returns `minOut`
already carrying your slippageBps floor (default 2%) / round-trip protection, so we submit it verbatim.

/submit accepts ONLY a fully-signed transaction whose `to` is the router and whose
calldata is a swapExactIn(...) call. The user signs it and pays their own gas; the
server never signs on your behalf. The approve above is NOT a swap, so it goes
through the wallet, never through /submit.

Env vars:
  PRIVATE_KEY  required 0x-hex key of the trader/recipient account (SECRET -> env only)
"""
import json
import os
import sys
import time
from decimal import Decimal

import requests
from eth_account import Account
from web3 import Web3

# --- Constants (verified on Arc testnet) -----------------------------------
CHAIN_ID = 5042002
BASE_TOKEN = "0x3600000000000000000000000000000000000000"  # native USDC / gas token

# Minimal ERC20 ABI: allowance + approve + decimals. decimals() lets us parse and
# display amounts correctly for any token (e.g. 6-decimal USDC), never assuming 18.
ERC20_ABI = json.loads("""
[
  {"name":"allowance","type":"function","stateMutability":"view",
   "inputs":[{"name":"owner","type":"address"},{"name":"spender","type":"address"}],
   "outputs":[{"name":"","type":"uint256"}]},
  {"name":"approve","type":"function","stateMutability":"nonpayable",
   "inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],
   "outputs":[{"name":"","type":"bool"}]},
  {"name":"decimals","type":"function","stateMutability":"view",
   "inputs":[],
   "outputs":[{"name":"","type":"uint8"}]}
]
""")

# Router ABI: only swapExactIn is needed to submit a signed authorization.
# auth is a struct, hops is an array of {tokenIn, tokenOut, legs[]} structs.
ROUTER_ABI = json.loads("""
[
  {"name":"swapExactIn","type":"function","stateMutability":"nonpayable",
   "inputs":[
     {"name":"auth","type":"tuple","components":[
       {"name":"trader","type":"address"},
       {"name":"tokenIn","type":"address"},
       {"name":"tokenOut","type":"address"},
       {"name":"amountIn","type":"uint256"},
       {"name":"minOut","type":"uint256"},
       {"name":"feeBps","type":"uint32"},
       {"name":"feeRecipient","type":"address"},
       {"name":"recipient","type":"address"},
       {"name":"deadline","type":"uint256"},
       {"name":"nonce","type":"uint256"},
       {"name":"routeHash","type":"bytes32"},
       {"name":"swapMode","type":"uint8"}
     ]},
     {"name":"signature","type":"bytes"},
     {"name":"hops","type":"tuple[]","components":[
       {"name":"tokenIn","type":"address"},
       {"name":"tokenOut","type":"address"},
       {"name":"legs","type":"tuple[]","components":[
         {"name":"adapter","type":"address"},
         {"name":"amountIn","type":"uint256"},
         {"name":"data","type":"bytes"}
       ]}
     ]}
   ],
   "outputs":[{"name":"netOut","type":"uint256"}]}
]
""")

# EIP-712 typed-data type list for the SwapAuthorization the API returns.
SWAP_AUTH_TYPES = [
    {"name": "trader", "type": "address"},
    {"name": "tokenIn", "type": "address"},
    {"name": "tokenOut", "type": "address"},
    {"name": "amountIn", "type": "uint256"},
    {"name": "minOut", "type": "uint256"},
    {"name": "feeBps", "type": "uint32"},
    {"name": "feeRecipient", "type": "address"},
    {"name": "recipient", "type": "address"},
    {"name": "deadline", "type": "uint256"},
    {"name": "nonce", "type": "uint256"},
    {"name": "routeHash", "type": "bytes32"},
    {"name": "swapMode", "type": "uint8"},
]


def env(name, default=None, required=False):
    val = os.environ.get(name, default)
    if required and not val:
        sys.exit(f"error: environment variable {name} is required")
    return val


def build_typed_data(auth, verifying_contract):
    """Assemble the full EIP-712 payload. The message IS the /swap `auth` object."""
    return {
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"},
            ],
            "SwapAuthorization": SWAP_AUTH_TYPES,
        },
        "primaryType": "SwapAuthorization",
        "domain": {
            "name": "MoonLite",
            "version": "1",
            "chainId": CHAIN_ID,
            "verifyingContract": Web3.to_checksum_address(verifying_contract),
        },
        # amountIn/minOut/deadline/nonce arrive as decimal strings; eth_account
        # coerces them to int for uint256. feeBps/swapMode arrive as numbers.
        "message": auth,
    }


def to_hops_tuples(hops):
    """Convert JSON hops into the ordered tuples web3 expects for the ABI."""
    out = []
    for h in hops:
        legs = [
            (Web3.to_checksum_address(l["adapter"]), int(l["amountIn"]), Web3.to_bytes(hexstr=l["data"]))
            for l in h["legs"]
        ]
        out.append((Web3.to_checksum_address(h["tokenIn"]),
                    Web3.to_checksum_address(h["tokenOut"]), legs))
    return out


def to_auth_tuple(auth):
    """Convert the auth dict into the ordered tuple matching the ABI struct."""
    return (
        Web3.to_checksum_address(auth["trader"]),
        Web3.to_checksum_address(auth["tokenIn"]),
        Web3.to_checksum_address(auth["tokenOut"]),
        int(auth["amountIn"]),
        int(auth["minOut"]),
        int(auth["feeBps"]),
        Web3.to_checksum_address(auth["feeRecipient"]),
        Web3.to_checksum_address(auth["recipient"]),
        int(auth["deadline"]),
        int(auth["nonce"]),
        Web3.to_bytes(hexstr=auth["routeHash"]),
        int(auth["swapMode"]),
    )


# Non-secret config - edit these. Only PRIVATE_KEY comes from the environment
# (it is a secret and must never live in source).
API_BASE = "https://api.moonlite.so"       # MOON.lite API base
RPC_URL = "https://api.moonlite.so/rpc"    # Arc testnet wallet JSON-RPC
TOKEN_IN = "0x3600000000000000000000000000000000000000"   # USDC (base / gas token)
TOKEN_OUT = "0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275"  # JUN (sample ERC20)
AMOUNT = "1"                               # human-readable amount of TOKEN_IN to swap
SLIPPAGE_BPS = 500                         # 5% - the router floors auth.minOut by this


def fmt(value, dec):
    """Format a base-unit integer amount as a human decimal string using `dec`."""
    return f"{Decimal(int(value)) / (Decimal(10) ** dec):f}"


def main():
    api_base = API_BASE.rstrip("/")
    rpc_url = RPC_URL
    private_key = env("PRIVATE_KEY", required=True)   # SECRET -> env only
    token_in = TOKEN_IN
    token_out = TOKEN_OUT

    acct = Account.from_key(private_key)
    trader = acct.address

    # web3 client (used to read decimals/allowance/nonce/fees and to send the approve).
    w3 = Web3(Web3.HTTPProvider(rpc_url))

    # Fetch ERC20 decimals on-chain for BOTH tokens before quoting. Never assume 18:
    # e.g. USDC is 6 decimals, so parsing/formatting must use the real value.
    dec_in = w3.eth.contract(
        address=Web3.to_checksum_address(token_in), abi=ERC20_ABI
    ).functions.decimals().call()
    dec_out = w3.eth.contract(
        address=Web3.to_checksum_address(token_out), abi=ERC20_ABI
    ).functions.decimals().call()

    # Scale the human-readable AMOUNT into TOKEN_IN base units using its real decimals.
    amount_in = str(int(Decimal(AMOUNT) * (Decimal(10) ** dec_in)))

    print(f"trader   : {trader}")
    print(f"api_base : {api_base}")
    print(f"decimals : in={dec_in} out={dec_out}")
    print(f"swap     : {AMOUNT} ({amount_in} base units) {token_in} -> {token_out}\n")

    # Per-step timers (milliseconds). approve stays 0 if allowance already covers it.
    t_quote = t_swap = t_sign = t_approve = t_submit = 0.0

    # 1) Preview quote (public, no auth). Amounts are integer strings, base units.
    _t0 = time.perf_counter()
    q = requests.get(api_base + "/quote", params={
        "tokenIn": token_in, "tokenOut": token_out, "amount": amount_in,
    }, timeout=10)
    q.raise_for_status()
    quote = q.json()
    t_quote = (time.perf_counter() - _t0) * 1000
    print("=== /quote preview ===")
    print(f"  amountIn      : {fmt(quote['amountIn'], dec_in)}")
    print(f"  amountOut     : {fmt(quote['amountOut'], dec_out)}")
    print(f"  priceImpactBps: {quote['priceImpactBps']}")
    print(f"  feeBps        : {quote['feeBps']}\n")

    # 2) Build the swap (public). trader = our address; NEVER send privateKey here.
    _t0 = time.perf_counter()
    s = requests.post(api_base + "/swap", json={
        "inputTokens": [token_in],
        "outputTokens": [token_out],
        "amount": amount_in,
        "trader": trader,
        "recipient": trader,
        "slippageBps": SLIPPAGE_BPS,
    }, timeout=15)
    s.raise_for_status()
    swap = s.json()
    t_swap = (time.perf_counter() - _t0) * 1000
    auth = swap["auth"]
    router_addr = Web3.to_checksum_address(swap["to"])
    print("=== /swap ===")
    print(f"  router (to): {router_addr}")
    print(f"  grossOut   : {fmt(swap['grossOut'], dec_out)}")
    print(f"  netOut     : {fmt(swap['netOut'], dec_out)}")
    print(f"  minOut     : {fmt(auth['minOut'], dec_out)}  (slippage/round-trip protected)\n")

    # 3) EIP-712 sign the returned auth with the user's wallet key.
    _t0 = time.perf_counter()
    typed = build_typed_data(auth, router_addr)
    signed = Account.sign_typed_data(private_key, full_message=typed)
    signature = signed.signature  # 65-byte r||s||v, v in {27,28}
    t_sign = (time.perf_counter() - _t0) * 1000
    # Sanity check: the digest we sign must equal the server-reported digest.
    if signed.message_hash.hex().lower().lstrip("0x") != swap["digest"].lower().lstrip("0x"):
        sys.exit(f"error: local digest {signed.message_hash.hex()} != server digest {swap['digest']}")
    print(f"signed digest matches server: {swap['digest']}\n")

    # 4) Approve the router to pull tokenIn if needed. approve is NOT a swap, so it
    #    goes through the wallet's own sender, never through /submit.
    _t0 = time.perf_counter()
    amount = int(amount_in)
    # The native base token needs no ERC20 approval; skip it for the gas token.
    if Web3.to_checksum_address(token_in) != Web3.to_checksum_address(BASE_TOKEN):
        erc20 = w3.eth.contract(address=Web3.to_checksum_address(token_in), abi=ERC20_ABI)
        allowance = erc20.functions.allowance(trader, router_addr).call()
        if allowance < amount:
            print(f"approving router for {fmt(amount, dec_in)} (current allowance {fmt(allowance, dec_in)})...")
            tx = erc20.functions.approve(router_addr, amount).build_transaction({
                "from": trader,
                "nonce": w3.eth.get_transaction_count(trader),
                "chainId": CHAIN_ID,
            })
            stx = acct.sign_transaction(tx)
            ah = w3.eth.send_raw_transaction(stx.raw_transaction)
            w3.eth.wait_for_transaction_receipt(ah)
            print(f"  approve tx: {ah.hex()}\n")
        else:
            print(f"allowance ok ({fmt(allowance, dec_in)} >= {fmt(amount, dec_in)})\n")
    t_approve = (time.perf_counter() - _t0) * 1000

    # 5) BUILD + locally SIGN the swapExactIn transaction, then POST the raw signed
    #    tx to /submit for faster inclusion. User's key, user's gas, user's nonce.
    _t0 = time.perf_counter()
    router = w3.eth.contract(address=router_addr, abi=ROUTER_ABI)
    base_fee = w3.eth.get_block("latest").get("baseFeePerGas") or 0
    max_priority = w3.eth.max_priority_fee
    max_fee = base_fee * 2 + max_priority
    swap_fn = router.functions.swapExactIn(
        to_auth_tuple(auth), signature, to_hops_tuples(swap["hops"]),
    )
    tx = swap_fn.build_transaction({
        "from": trader,
        "nonce": w3.eth.get_transaction_count(trader),
        "gas": swap_fn.estimate_gas({"from": trader}),
        "maxFeePerGas": max_fee,
        "maxPriorityFeePerGas": max_priority,
        "chainId": CHAIN_ID,
    })
    stx = acct.sign_transaction(tx)
    raw_hex = stx.raw_transaction.hex()
    if not raw_hex.startswith("0x"):
        raw_hex = "0x" + raw_hex
    r = requests.post(api_base + "/submit", json={"rawTx": raw_hex}, timeout=15)
    r.raise_for_status()
    res = r.json()
    t_submit = (time.perf_counter() - _t0) * 1000
    if not res.get("ok"):
        sys.exit(f"error: /submit rejected the transaction: {res}")
    tx_hash = res["txHash"]
    print("=== /submit ===")
    print(f"  txHash: {tx_hash}\n")

    total = t_quote + t_swap + t_sign + t_approve + t_submit
    print("=== timing ===")
    print(f"  quote    :  {t_quote:.1f} ms")
    print(f"  /swap    :  {t_swap:.1f} ms")
    print(f"  sign     :  {t_sign:.1f} ms")
    print(f"  approve  :  {t_approve:.1f} ms")
    print(f"  /submit  :  {t_submit:.1f} ms")
    print(f"  total    :  {total:.1f} ms")


if __name__ == "__main__":
    main()
