#!/usr/bin/env python3
"""
MOON.lite router aggregator - minimal end-to-end Python sample client.

Flow (see README):
  1) GET  /quote  -> print a human-readable preview
  2) POST /swap   -> get {auth, hops, to, netOut}  (trader = your address, NO privateKey)
  3) EIP-712 sign the returned `auth` (SwapAuthorization) with your local key
  4) ERC20 approve(router, amountIn) on tokenIn if allowance < amountIn
  5) call router.swapExactIn(auth, signature, hops) at `to`, print tx hash + receipt

The API is public: /quote and /swap need no auth. The server returns `minOut`
already carrying slippage / round-trip protection, so we submit it verbatim.

Env vars:
  API_BASE     default http://127.0.0.1:8088   (public testnet base is configurable)
  RPC_URL      default http://127.0.0.1:8545    (Arc testnet, chainId 5042002)
  PRIVATE_KEY  required 0x-hex key of the trader/recipient account
  TOKEN_IN     default 0x3600...0000 (native USDC base token / gas token)
  TOKEN_OUT    required output token address (see GET /tokens for the list)
  AMOUNT_IN    input amount in TOKEN_IN base units (wei-like); default 1e18
"""
import json
import os
import sys

import requests
from eth_account import Account
from web3 import Web3

# --- Constants (verified on Arc testnet) -----------------------------------
CHAIN_ID = 5042002
BASE_TOKEN = "0x3600000000000000000000000000000000000000"  # native USDC / gas token

# Minimal ERC20 ABI: allowance + approve are all we need on the input token.
ERC20_ABI = json.loads("""
[
  {"name":"allowance","type":"function","stateMutability":"view",
   "inputs":[{"name":"owner","type":"address"},{"name":"spender","type":"address"}],
   "outputs":[{"name":"","type":"uint256"}]},
  {"name":"approve","type":"function","stateMutability":"nonpayable",
   "inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],
   "outputs":[{"name":"","type":"bool"}]}
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


def main():
    api_base = env("API_BASE", "http://127.0.0.1:8088").rstrip("/")
    rpc_url = env("RPC_URL", "http://127.0.0.1:8545")
    private_key = env("PRIVATE_KEY", required=True)
    token_in = env("TOKEN_IN", BASE_TOKEN)
    token_out = env("TOKEN_OUT", required=True)
    amount_in = env("AMOUNT_IN", str(10**18))

    acct = Account.from_key(private_key)
    trader = acct.address
    print(f"trader   : {trader}")
    print(f"api_base : {api_base}")
    print(f"swap     : {amount_in} {token_in} -> {token_out}\n")

    # 1) Preview quote (public, no auth). Amounts are integer strings, base units.
    q = requests.get(api_base + "/quote", params={
        "tokenIn": token_in, "tokenOut": token_out, "amount": amount_in,
    }, timeout=10)
    q.raise_for_status()
    quote = q.json()
    print("=== /quote preview ===")
    print(f"  amountIn      : {quote['amountIn']}")
    print(f"  amountOut     : {quote['amountOut']}")
    print(f"  priceImpactBps: {quote['priceImpactBps']}")
    print(f"  feeBps        : {quote['feeBps']}\n")

    # 2) Build the swap (public). trader = our address; NEVER send privateKey here.
    s = requests.post(api_base + "/swap", json={
        "inputTokens": [token_in],
        "outputTokens": [token_out],
        "amount": amount_in,
        "trader": trader,
        "recipient": trader,
    }, timeout=15)
    s.raise_for_status()
    swap = s.json()
    auth = swap["auth"]
    router_addr = Web3.to_checksum_address(swap["to"])
    print("=== /swap ===")
    print(f"  router (to): {router_addr}")
    print(f"  grossOut   : {swap['grossOut']}")
    print(f"  netOut     : {swap['netOut']}")
    print(f"  minOut     : {auth['minOut']}  (slippage/round-trip protected)\n")

    # 3) EIP-712 sign the returned auth with the user's wallet key.
    typed = build_typed_data(auth, router_addr)
    signed = Account.sign_typed_data(private_key, full_message=typed)
    signature = signed.signature  # 65-byte r||s||v, v in {27,28}
    # Sanity check: the digest we sign must equal the server-reported digest.
    if signed.message_hash.hex().lower().lstrip("0x") != swap["digest"].lower().lstrip("0x"):
        sys.exit(f"error: local digest {signed.message_hash.hex()} != server digest {swap['digest']}")
    print(f"signed digest matches server: {swap['digest']}\n")

    # 4) Connect web3 and approve the router to pull tokenIn if needed.
    w3 = Web3(Web3.HTTPProvider(rpc_url))
    amount = int(amount_in)
    # The native base token needs no ERC20 approval; skip it for the gas token.
    if Web3.to_checksum_address(token_in) != Web3.to_checksum_address(BASE_TOKEN):
        erc20 = w3.eth.contract(address=Web3.to_checksum_address(token_in), abi=ERC20_ABI)
        allowance = erc20.functions.allowance(trader, router_addr).call()
        if allowance < amount:
            print(f"approving router for {amount} (current allowance {allowance})...")
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
            print(f"allowance ok ({allowance} >= {amount})\n")

    # 5) Submit swapExactIn(auth, signature, hops) to the router.
    router = w3.eth.contract(address=router_addr, abi=ROUTER_ABI)
    tx = router.functions.swapExactIn(
        to_auth_tuple(auth), signature, to_hops_tuples(swap["hops"]),
    ).build_transaction({
        "from": trader,
        "nonce": w3.eth.get_transaction_count(trader),
        "chainId": CHAIN_ID,
    })
    stx = acct.sign_transaction(tx)
    txh = w3.eth.send_raw_transaction(stx.raw_transaction)
    print(f"swap tx sent: {txh.hex()}")
    rcpt = w3.eth.wait_for_transaction_receipt(txh)
    print(f"status: {'SUCCESS' if rcpt.status == 1 else 'FAILED'}  block {rcpt.blockNumber}  gas {rcpt.gasUsed}")


if __name__ == "__main__":
    main()
