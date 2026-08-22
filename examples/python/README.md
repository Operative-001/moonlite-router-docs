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

Only your **secret key** comes from the environment (`PRIVATE_KEY`). Everything
else is a constant at the top of `main.py` — edit the config block there.

| setting | source | default | notes |
|---|---|---|---|
| `PRIVATE_KEY` | **env** | (required) | secret — `os.environ["PRIVATE_KEY"]`, never hardcode |
| `API_BASE` | `main.py` | `https://api.moonlite.so` | MOON.lite API base |
| `RPC_URL` | `main.py` | `https://api.moonlite.so/rpc` | Arc testnet JSON-RPC |
| `TOKEN_IN` | `main.py` | USDC `0x3600…0000` | input token |
| `TOKEN_OUT` | `main.py` | `0xa4a3…2275` (JUN) | output token (see `GET /tokens`) |
| `AMOUNT_IN` | `main.py` | `1000000000000000000` | input amount in base units |
| `SLIPPAGE_BPS` | `main.py` | `500` | max price movement (5%); floors `auth.minOut` |

## Run

```bash
export PRIVATE_KEY=0x...   # secret — signs locally, never sent to the API
python main.py             # edit tokens/amount/slippage at the top of main.py
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
