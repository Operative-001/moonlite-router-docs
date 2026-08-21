# MOON.lite — Node sample client

Minimal, runnable client that quotes and executes a swap through the MOON.lite
router aggregator on Arc testnet using [viem](https://viem.sh).

## Flow

1. `GET /quote` — preview `amountOut`, `priceImpactBps`, `feeBps` (public, no auth).
2. `POST /swap` — build the best route; returns `{ auth, hops, to, netOut, digest }`.
3. Sign the `auth` object as an EIP-712 `SwapAuthorization` with your wallet.
4. `approve(router, amountIn)` on `tokenIn` if the allowance is short.
5. `router.swapExactIn(auth, signature, hops)` at the returned `to`; wait for the receipt.

`minOut` returned by the API already includes slippage / round-trip protection.

## Run

```bash
npm i
export PRIVATE_KEY=0x...            # your wallet key (used to SIGN locally, never sent to the API)
export RPC_URL=http://127.0.0.1:8545
export API_BASE=http://127.0.0.1:8088
node index.js
```

## Environment

| var | default | notes |
| --- | --- | --- |
| `PRIVATE_KEY` | (required) | 0x-prefixed 32-byte hex; signs EIP-712 + sends txs |
| `API_BASE` | `http://127.0.0.1:8088` | MOON.lite API base; configurable (public testnet TBD) |
| `RPC_URL` | `http://127.0.0.1:8545` | Arc testnet JSON-RPC (chainId `5042002`) |
| `TOKEN_IN` | base/native USDC `0x3600…0000` | input token |
| `TOKEN_OUT` | `0xa4a3…2275` (JUN) | output token; see `GET /tokens` for the full list |
| `AMOUNT` | `1000000000000000000` | integer base units (wei-like) of `TOKEN_IN` |
| `RECIPIENT` | trader | optional output recipient |

## Notes

- The quote/swap plane is **public** — no API token needed.
- We **never** send `privateKey` to the server. The signature is produced locally by
  your wallet. (Passing `privateKey` to `/swap` is a backend-only convenience and is
  intentionally omitted here.)
- The EIP-712 `verifyingContract` is the `to` address returned by `/swap`
  (`0xFECBFfCa1394545d3fe6620DFA4Fd3C8E3754E4B` on the current testnet).
- Amounts are integer strings in each token`s own base units (per its `decimals`).
