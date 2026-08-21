# MOON.lite — Rust sample client

A standalone, dependency-light Rust client for the **MOON.lite router aggregator**
on the **Arc testnet** (chainId `5042002`). It runs the full swap flow:

1. `GET /quote` — print a price preview.
2. `POST /swap` — get the signed-order payload `{auth, hops, to, netOut}`.
   We send `trader` = our signer address and **never** send `privateKey`
   (that field is backend-only; a real client always signs locally).
3. Reconstruct the **EIP-712 `SwapAuthorization`** domain + struct from the
   response and sign `auth` with the local key (`v` normalised to 27/28).
4. `approve(router, amountIn)` on `tokenIn` if the allowance is too low.
5. Submit `router.swapExactIn(auth, signature, hops)` to the `to` address and
   print the transaction hash.

`minOut` in the `auth` already carries slippage / round-trip protection computed
by the API, so the client passes it through verbatim.


## Dependencies

- [`reqwest`](https://crates.io/crates/reqwest) — async HTTP (`/quote`, `/swap`)
- [`serde_json`](https://crates.io/crates/serde_json) — JSON
- [`alloy`](https://crates.io/crates/alloy) `2.4.1` — EIP-712 signing (`sol!` +
  `SolStruct`), ABI encoding, local signer, and the JSON-RPC provider. The
  default features already include the `sol!` macro, an HTTP provider, contracts,
  and a local private-key signer.
- [`tokio`](https://crates.io/crates/tokio) — async runtime
- [`anyhow`](https://crates.io/crates/anyhow) — error handling

## Configuration (env vars)

| Var           | Required | Default                                        | Meaning                                            |
| ------------- | -------- | ---------------------------------------------- | -------------------------------------------------- |
| `PRIVATE_KEY` | yes      | —                                              | 0x-hex private key of the trader wallet            |
| `TOKEN_OUT`   | yes      | —                                              | tokenOut address                                   |
| `API_BASE`    | no       | `http://127.0.0.1:8088`                        | MOON.lite API base (public swap/quote plane)       |
| `RPC_URL`     | no       | `http://127.0.0.1:8545`                        | Arc testnet JSON-RPC endpoint                      |
| `TOKEN_IN`    | no       | `0x3600000000000000000000000000000000000000`   | tokenIn address (defaults to native USDC / gas)    |
| `AMOUNT_IN`   | no       | `1000000000000000000`                          | amount in tokenIn base units (wei-like), integer   |

Amounts are **integer strings in token base units** (per-token decimals).

## Run

```sh
# minimum: a funded key + a destination token
export PRIVATE_KEY=0xabc...                                   # your trader wallet
export TOKEN_OUT=0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275   # any token from GET /tokens
cargo run --release
```

Full example (all knobs explicit, pointing at a local node):

```sh
API_BASE=http://127.0.0.1:8088 \
RPC_URL=http://127.0.0.1:8545 \
PRIVATE_KEY=0xabc... \
TOKEN_IN=0x3600000000000000000000000000000000000000 \
TOKEN_OUT=0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275 \
AMOUNT_IN=1000000000000000000 \
cargo run --release
```

Against a hosted deployment, point `API_BASE`/`RPC_URL` at the published testnet
endpoints (configurable; e.g. `https://api.moonlite.example`).

Discover tradable tokens (address / symbol / name / decimals) with:

```sh
curl "$API_BASE/tokens"
```

## Expected output

```
trader (signer) = 0x....
quote: "1000000000000000000" 0x3600... -> "399191381835141678471" 0xa4a3... (priceImpactBps=0, feeBps=10)
swap: router=0xFECBFfCa1394545d3fe6620DFA4Fd3C8E3754E4B  grossOut="..."  netOut="..."
eip712 digest OK: 0x....
allowance already sufficient (...); skipping approve
swap tx submitted: 0x....
swap tx mined: 0x....
```

The client cross-checks its **locally computed EIP-712 digest** against the
server-returned `digest` before signing; a mismatch aborts the run.

## EIP-712 domain & type (for reference)

```
domain = { name: "MoonLite", version: "1", chainId: 5042002, verifyingContract: <to from /swap> }

SwapAuthorization(
  address trader, address tokenIn, address tokenOut,
  uint256 amountIn, uint256 minOut,
  uint32 feeBps, address feeRecipient, address recipient,
  uint256 deadline, uint256 nonce, bytes32 routeHash, uint8 swapMode
)
```

`signature` is 65 bytes `r || s || v`, `v` in `{27, 28}`.
