# MOON.lite

**A router / aggregator engine for the Arc blockchain — built for speed and reliability.**

[![release](https://img.shields.io/badge/release-v0.1.0--beta-orange)](../../releases)
[![status](https://img.shields.io/badge/status-beta-yellow)](#)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![chain](https://img.shields.io/badge/chain-Arc%20testnet%20%285042002%29-8957e5)](#network)
[![pools](https://img.shields.io/badge/pools-45%2C153-2ea043)](#coverage)
[![DEX families](https://img.shields.io/badge/DEX%20families-7-1f6feb)](#coverage)
[![tokens](https://img.shields.io/badge/tokens-34%2C196-8957e5)](#coverage)
[![median quote](https://img.shields.io/badge/median%20quote-~0.7ms-db61a2)](#performance)

MOON.lite indexes **45,153 pools** across **7 DEX families** spanning **34,196 tokens**, and returns best-execution routes in **sub-millisecond** time. Give it a token in and a token out; it finds the optimal path — split across pools, chained across hops — and hands back a single EIP-712 order your wallet signs and submits.

Targets **Arc testnet** (chainId `5042002`) today. The API base URL is configurable.

---

## 30-second quickstart

Get a live quote — public, no auth. Amounts are integer base-unit strings (wei-like, per token decimals):

```bash
curl "$API_BASE/quote?tokenIn=0x3600000000000000000000000000000000000000&tokenOut=0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275&amount=1000000000000000000"
```

```json
{
  "tokenIn": "0x3600000000000000000000000000000000000000",
  "tokenOut": "0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275",
  "direction": "ExactIn",
  "amount": "1000000000000000000",
  "amountIn": "1000000000000000000",
  "amountOut": "399191495856262490842",
  "priceImpactBps": 0,
  "feeBps": 10
}
```

Ready to trade? Build and sign a swap in five steps — runnable clients in **[Rust](examples/rust)**, **[Python](examples/python)**, and **[Node](examples/node)**.

> `$API_BASE` is a placeholder for your endpoint (e.g. `https://api.moonlite.example`, or `http://127.0.0.1:8088` for a local node).

---

## Coverage

| DEX family | Kind | Pools |
|---|---|---:|
| **UniswapV2 forks** | `v2` | 45,044 |
| **UniswapV3 / concentrated liquidity** | `v3` | 69 |
| **Curve / StableSwap** | `curve` | 31 |
| **ArcDex** | `arcdex` | 5 |
| **Xylo** | `xylo` | 2 |
| **FixedPair** | `fixedpair` | 1 |
| **SwapArc** | `swaparc` | 1 |
| **Total** | | **45,153** |

**34,196** distinct tokens appear across those pools. Every pool carries an on-chain **interface schema** so the engine knows exactly how to read reserves and call the swap selector — no guessing, no ABI drift.

---

## Performance

| | |
|---|---|
| **Median quote** | ~0.7 ms (end-to-end, including HTTP over localhost) |
| **Routing** | Multi-hop, multi-leg — a single trade is split across pools and chained across hops for best execution |
| **Certified-tradeable venues** | **341** live right now — see [`/health`](docs/API.md#get-health) |

Every venue must pass a **live on-chain adapter round-trip certification** before it can enter a route. Uncertified or failing venues are fenced out at quote time, so a route you get back is a route that executes.

---

## Multi-hop & round-trip routing

`POST /swap` takes **`inputTokens[]`** and **`outputTokens[]`** arrays. Swap `i` routes `inputTokens[i] → outputTokens[i]` via the engine's best split route; the output of each hop feeds the next, and the whole chain comes back as **one signed transaction**.

```jsonc
// A → B → C in one signed tx (each hop optimally split across pools)
{ "inputTokens":  ["0xA…", "0xB…"],
  "outputTokens": ["0xB…", "0xC…"],
  "amount": "1000000000000000000",
  "trader": "0x…" }
```

If the chain **ends where it began** (`outputTokens[last] == inputTokens[0]`), the order switches to **profit-only** mode: the router requires `netOut > amountIn`, so a cyclic route either lands in profit or reverts — never at a loss. Full semantics in **[docs/API.md](docs/API.md#post-swap)**.

---

## How it works

1. **`ERC20.approve(router, amountIn)`** on `tokenIn`.
2. **`GET /quote`** — preview `amountOut`, price impact, and fee.
3. **`POST /swap`** — receive a signed-authorization payload: `auth`, `hops`, the router address (`to`), and `netOut`. `minOut` already carries slippage / round-trip protection.
4. **Sign** the `auth` object via EIP-712 with the user's wallet — keys never leave the client.
5. **`router.swapExactIn(auth, signature, hops)`** — submit to `to`.

Full walkthrough in **[docs/SIGNING.md](docs/SIGNING.md)**.

---

## Documentation

- **[docs/API.md](docs/API.md)** — full endpoint reference: `/health`, `/quote`, `/swap`, `/tokens`, `/venues`.
- **[docs/SIGNING.md](docs/SIGNING.md)** — EIP-712 `SwapAuthorization` signing guide + router ABI.
- **[examples/](examples/)** — runnable clients: [Rust](examples/rust) · [Python](examples/python) · [Node](examples/node).
- **[CHANGELOG.md](CHANGELOG.md)** — release history.

---

## Network

| | |
|---|---|
| **Chain** | Arc testnet |
| **chainId** | `5042002` (`0x4cef52`) |
| **Router** (EIP-712 `verifyingContract` and submit target) | `0xFECBFfCa1394545d3fe6620DFA4Fd3C8E3754E4B` |
| **Base token** (native USDC, gas token) | `0x3600000000000000000000000000000000000000` |
| **Protocol fee** | 10 bps, applied and returned transparently in every quote |
| **API base** | configurable (placeholder `https://api.moonlite.example`; local node `http://127.0.0.1:8088`) |

---

## License

[MIT](LICENSE) — applies to the documentation and the sample client code in this repository.
