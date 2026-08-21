# MOON.lite - Python sample client

Minimal, runnable end-to-end client for the MOON.lite router aggregator on the
Arc testnet (chainId `5042002`). It quotes a swap, asks the API to build the
route, EIP-712-signs the returned authorization with your local key, approves
the router if needed, and submits `swapExactIn` on-chain.

The quote/swap plane is **public** - no API token required. The server returns a
`minOut` that already carries slippage / round-trip protection, so the client
submits it verbatim.

## Install

```bash
pip install -r requirements.txt
```

## Configure

| Env var       | Required | Default                                      | Notes |
|---------------|----------|----------------------------------------------|-------|
| `PRIVATE_KEY` | yes      | -                                            | `0x`-hex key of the trader/recipient. |
| `TOKEN_OUT`   | yes      | -                                            | Output token address (see `GET /tokens`). |
| `API_BASE`    | no       | `http://127.0.0.1:8088`                      | Public testnet base is configurable, e.g. `https://api.moonlite.example`. |
| `RPC_URL`     | no       | `http://127.0.0.1:8545`                      | Arc testnet JSON-RPC. |
| `TOKEN_IN`    | no       | `0x3600000000000000000000000000000000000000` | Native USDC base token (gas token). |
| `AMOUNT_IN`   | no       | `1000000000000000000`                        | Input amount in `TOKEN_IN` base units (wei-like). |

Discover tokens:

```bash
curl -s "$API_BASE/tokens" | python3 -m json.tool
```

## Run

```bash
export PRIVATE_KEY=0x...
export TOKEN_OUT=0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275   # example: JUN
python main.py
```

## What it does

1. `GET /quote` - prints `amountOut`, `priceImpactBps`, `feeBps` (10 bps protocol fee).
2. `POST /swap` with `trader = your address` (and **no** `privateKey`) - returns
   `{auth, hops, to, netOut, digest}`. `to` is the router / EIP-712
   `verifyingContract`.
3. EIP-712 signs the `auth` (`SwapAuthorization`) via `eth_account`, producing a
   65-byte `r||s||v` signature, and asserts the local digest matches the
   server's `digest`.
4. ERC20 `approve(router, amountIn)` on `TOKEN_IN` if allowance is short
   (skipped for the native base token).
5. Sends `router.swapExactIn(auth, signature, hops)` and prints the tx hash and
   receipt status.

## Security note

Never send your `privateKey` to the API. The `/swap` endpoint can optionally
sign server-side, but that path is **backend-only**. A wallet/browser/CLI client
must omit `privateKey` and sign locally, exactly as this sample does.
