# Changelog

All notable changes to the MOON.lite public interface (API + sample clients) are documented here. This project follows [Semantic Versioning](https://semver.org).

## [0.1.0-beta] — 2026-08-21

First public beta of the MOON.lite router-aggregator interface for **Arc testnet** (chainId `5042002`).

### Added
- **Public quote/swap plane** — `GET /quote`, `POST /swap`, `GET /tokens`, `GET /health` (no auth). `GET /venues` is Bearer-gated.
- **`ExactIn` and `ExactOut` quoting** on `GET /quote`.
- **Multi-hop & round-trip routing** — `POST /swap` accepts `inputTokens[]` / `outputTokens[]`; hops chain into one signed transaction, and a cyclic route runs in profit-only mode (`netOut > amountIn` or revert).
- **EIP-712 `SwapAuthorization`** signing flow — the API prices and shapes the route; the wallet signs. Keys never leave the client.
- **On-chain token directory** — `symbol` / `name` / `decimals` resolved on-chain the first time a token is seen, served via `GET /tokens`.
- **Certification & interface-schema telemetry** on `GET /health`.
- **Sample clients** — runnable Rust, Python, and Node integrations covering the full quote → sign → submit flow.
- **Docs** — full API reference (`docs/API.md`) and EIP-712 signing guide (`docs/SIGNING.md`).

### Coverage at release
- **45,153 pools** across **7 DEX families** (UniswapV2 forks, UniswapV3, Curve, ArcDex, Xylo, FixedPair, SwapArc).
- **34,196 tokens** across all pools; **341 certified-tradeable venues** live.
- **~0.7 ms** median quote latency.

[0.1.0-beta]: https://github.com/Operative-001/moonlite-router-docs/releases/tag/v0.1.0-beta
