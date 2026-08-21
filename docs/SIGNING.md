# MOON.lite — Signing Guide (EIP-712 `SwapAuthorization`)

To execute a swap, the user's wallet signs a typed `SwapAuthorization` message and submits it to the router. The API **prices and shapes the route**; the **user's key signs**. For browser / wallet clients the private key **never leaves the client** — do not send `privateKey` to `/swap`.

The message you sign is exactly the **`auth` object returned by `POST /swap`**.

---

## The EIP-712 domain

```json
{
  "name": "MoonLite",
  "version": "1",
  "chainId": 5042002,
  "verifyingContract": "<to from /swap>"
}
```

`verifyingContract` is the router — the `to` field returned by `/swap` (on Arc testnet: `0xFECBFfCa1394545d3fe6620DFA4Fd3C8E3754E4B`). Always take it from the response so the domain stays correct if the deployment changes.

---

## The type

```js
const types = {
  SwapAuthorization: [
    { name: "trader",        type: "address" },
    { name: "tokenIn",       type: "address" },
    { name: "tokenOut",      type: "address" },
    { name: "amountIn",      type: "uint256" },
    { name: "minOut",        type: "uint256" },
    { name: "feeBps",        type: "uint32"  },
    { name: "feeRecipient",  type: "address" },
    { name: "recipient",     type: "address" },
    { name: "deadline",      type: "uint256" },
    { name: "nonce",         type: "uint256" },
    { name: "routeHash",     type: "bytes32" },
    { name: "swapMode",      type: "uint8"   }
  ]
};
```

---

## The message

The message **is** the `auth` object from `/swap`, passed through unchanged:

```json
{
  "trader": "0x1111111111111111111111111111111111111111",
  "tokenIn": "0x3600000000000000000000000000000000000000",
  "tokenOut": "0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275",
  "amountIn": "1000000000000000000",
  "minOut": "390816456271584736386",
  "feeBps": 10,
  "feeRecipient": "0x088ef1acbcc46a522ab57190f89fb002d68b38d7",
  "recipient": "0x1111111111111111111111111111111111111111",
  "deadline": "1787271748",
  "nonce": "0",
  "routeHash": "0x9e89d9716da011380bb0cee25f885b5ebb7eed007423d7bb165d8e769e03b58f",
  "swapMode": 0
}
```

Field types when signing:

- `amountIn`, `minOut`, `deadline`, `nonce` — **decimal strings** (`uint256`); pass as-is (viem/ethers accept strings or BigInt).
- `feeBps` (`uint32`), `swapMode` (`uint8`) — **numbers**.
- `routeHash` — `bytes32` hex.
- addresses — hex.

`minOut` **already carries slippage / round-trip protection** from the API — do not modify it. Do not edit any field before signing: the router validates against `routeHash` and the digest.

> **Cross-check:** `/swap` returns `digest` — the EIP-712 digest of this message. Your client's typed-data hash must equal it before you sign.

---

## The signature

A standard **65-byte `r || s || v`** ECDSA signature, with **`v` in {27, 28}** — exactly what `signTypedData` (viem / ethers) and `eth_account.sign_typed_data` (eth-account) produce.

- **viem:** `walletClient.signTypedData({ account, domain, types, primaryType: "SwapAuthorization", message: auth })`
- **ethers v6:** `signer.signTypedData(domain, types, auth)`
- **eth-account (Python):** `Account.sign_typed_data(private_key, full_message={ ... })` → use `.signature`

---

## The 5-step client flow

1. **Approve.** `ERC20.approve(router, amountIn)` on `tokenIn` (`router` = `to` from `/swap`).
2. **Quote.** `GET /quote` for the price preview.
3. **Build.** `POST /swap` → `{ auth, hops, to, netOut, digest }`. (Wallet clients omit `privateKey`.)
4. **Sign.** EIP-712-sign the `auth` message with the user's wallet → 65-byte `signature`.
5. **Execute.** Send `router.swapExactIn(auth, signature, hops)` to `to`.

---

## Router ABI (submit)

```solidity
struct SwapAuthorization {
    address trader;
    address tokenIn;
    address tokenOut;
    uint256 amountIn;
    uint256 minOut;
    uint32  feeBps;
    address feeRecipient;
    address recipient;
    uint256 deadline;
    uint256 nonce;
    bytes32 routeHash;
    uint8   swapMode;
}

struct Leg { address adapter; uint256 amountIn; bytes data; }
struct Hop { address tokenIn; address tokenOut; Leg[] legs; }

// Fixed route (the hops returned by /swap):
function swapExactIn(
    SwapAuthorization calldata auth,
    bytes calldata signature,
    Hop[] calldata hops
) external returns (uint256 netOut);

// JIT best-route-at-execution over the candidate graph:
function swapCandidateGraph(
    SwapAuthorization calldata auth,
    bytes calldata signature,
    Hop[][] calldata candidates
) external returns (uint256 netOut);
```

Human-readable ABI (viem / ethers):

```
function swapExactIn((address trader,address tokenIn,address tokenOut,uint256 amountIn,uint256 minOut,uint32 feeBps,address feeRecipient,address recipient,uint256 deadline,uint256 nonce,bytes32 routeHash,uint8 swapMode) auth, bytes signature, (address tokenIn,address tokenOut,(address adapter,uint256 amountIn,bytes data)[] legs)[] hops) returns (uint256 netOut)
```

- Pass `auth` and `hops` **exactly as returned by `/swap`** — the router re-derives `routeHash` and verifies the signature against it.
- `swapExactIn` executes the fixed route in `hops`. `swapCandidateGraph` takes the `candidates` graph and picks the best route at execution time (JIT), which can improve fills when pool state moves between quoting and inclusion.

---

## See also

- **[API.md](API.md)** — endpoint reference (`/quote`, `/swap`, …).
- **Runnable examples** — [Rust](../examples/rust) · [Python](../examples/python) · [Node](../examples/node).
