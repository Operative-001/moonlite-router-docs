//! MOON.lite router-aggregator — standalone Rust sample client.
//!
//! End-to-end flow (see the MOON.lite spec):
//!   1) GET  /quote  -> print a human-readable preview.
//!   2) POST /swap   -> get {auth, hops, to, netOut}. We send `trader` = our
//!                      signer address and DO NOT send `privateKey` (that field is
//!                      backend-only; a real client always signs locally).
//!   3) Rebuild the EIP-712 `SwapAuthorization` domain+struct from the response and
//!      sign the `auth` object with our local key (v is normalised to 27/28).
//!   4) ERC20 approve(router, amountIn) on tokenIn if the allowance is too low.
//!   5) Submit router.swapExactIn(auth, signature, hops) to the `to` address and
//!      print the transaction hash. `minOut` already carries slippage / round-trip
//!      protection computed by the API, so we pass the auth through verbatim.
//!
//! Config via env vars:
//!   API_BASE     e.g. http://127.0.0.1:8088   (MOON.lite API; default shown)
//!   RPC_URL      e.g. http://127.0.0.1:8545   (Arc testnet JSON-RPC; default shown)
//!   PRIVATE_KEY  0x-prefixed 32-byte hex key of the trader wallet (required)
//!   TOKEN_IN     tokenIn address  (default = native USDC base token)
//!   TOKEN_OUT    tokenOut address (required)
//!   AMOUNT_IN    integer amount in tokenIn base units (wei-like) (default 1e18)

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, FixedBytes, Signature, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolStruct};
use anyhow::{anyhow, Context, Result};
use std::env;
use std::str::FromStr;

// Arc testnet.
const CHAIN_ID: u64 = 5042002;
// Native USDC / gas token — the default tokenIn.
const BASE_TOKEN: &str = "0x3600000000000000000000000000000000000000";

// -- Solidity types, shared by EIP-712 signing AND by ABI-encoding the tx. --------
//
// `SwapAuthorization` MUST match the EIP-712 type list exactly (name/type/order);
// the sol! macro derives the correct EIP-712 typeHash from this definition. The same
// struct is reused as the first argument of swapExactIn, and Leg/Hop mirror the
// `hops` array returned by /swap. `#[sol(rpc)]` on the interface generates a typed,
// signer-aware contract binding we can `.call()` (reads) and `.send()` (writes).
sol! {
    #[allow(missing_docs)]
    #[derive(Debug)]
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

    #[allow(missing_docs)]
    struct Leg { address adapter; uint256 amountIn; bytes data; }

    #[allow(missing_docs)]
    struct Hop { address tokenIn; address tokenOut; Leg[] legs; }

    #[allow(missing_docs)]
    #[sol(rpc)]
    contract Router {
        function swapExactIn(SwapAuthorization auth, bytes signature, Hop[] hops)
            external returns (uint256 netOut);
    }

    #[allow(missing_docs)]
    #[sol(rpc)]
    contract Erc20 {
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 amount) external returns (bool);
    }
}

// -- small env / parse helpers ----------------------------------------------------

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn addr(s: &str) -> Result<Address> {
    Address::from_str(s.trim()).with_context(|| format!("bad address: {s}"))
}

/// The /swap `auth` object encodes addresses/amountIn/minOut/deadline/nonce as
/// JSON strings, feeBps/swapMode as JSON numbers, and routeHash as 0x-bytes32.
fn u256_from_json(v: &serde_json::Value, field: &str) -> Result<U256> {
    let s = v
        .get(field)
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("auth.{field} missing or not a string"))?;
    U256::from_str(s).with_context(|| format!("auth.{field} not an integer: {s}"))
}

fn addr_from_json(v: &serde_json::Value, field: &str) -> Result<Address> {
    let s = v
        .get(field)
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("auth.{field} missing or not a string"))?;
    addr(s)
}

fn u64_from_json(v: &serde_json::Value, field: &str) -> Result<u64> {
    v.get(field)
        .and_then(|x| x.as_u64())
        .ok_or_else(|| anyhow!("auth.{field} missing or not a number"))
}

#[tokio::main]
async fn main() -> Result<()> {
    // ---- config -----------------------------------------------------------------
    let api_base = env_or("API_BASE", "http://127.0.0.1:8088");
    let rpc_url = env_or("RPC_URL", "http://127.0.0.1:8545");
    let pk = env::var("PRIVATE_KEY").context("PRIVATE_KEY env var is required")?;
    let token_in = env_or("TOKEN_IN", BASE_TOKEN);
    let token_out = env::var("TOKEN_OUT").context("TOKEN_OUT env var is required")?;
    let amount_in = env_or("AMOUNT_IN", "1000000000000000000"); // 1e18 base units

    let signer: PrivateKeySigner = pk.trim().parse().context("invalid PRIVATE_KEY")?;
    let trader = signer.address();
    println!("trader (signer) = {trader}");

    let http = reqwest::Client::new();

    // ---- 1) quote preview -------------------------------------------------------
    let quote_url = format!(
        "{api_base}/quote?tokenIn={token_in}&tokenOut={token_out}&amount={amount_in}"
    );
    let quote: serde_json::Value = http
        .get(&quote_url)
        .send()
        .await?
        .error_for_status()
        .context("GET /quote failed")?
        .json()
        .await?;
    println!(
        "quote: {} {} -> {} {}  (priceImpactBps={}, feeBps={})",
        quote["amountIn"], token_in, quote["amountOut"], token_out,
        quote["priceImpactBps"], quote["feeBps"],
    );

    // ---- 2) build the swap ------------------------------------------------------
    // Single swap: inputTokens=[tokenIn], outputTokens=[tokenOut]. NO privateKey.
    let swap_req = serde_json::json!({
        "inputTokens":  [token_in],
        "outputTokens": [token_out],
        "amount":       amount_in,
        "trader":       trader.to_string(),
    });
    let swap: serde_json::Value = http
        .post(format!("{api_base}/swap"))
        .json(&swap_req)
        .send()
        .await?
        .error_for_status()
        .context("POST /swap failed")?
        .json()
        .await?;

    let router: Address = addr(
        swap["to"].as_str().ok_or_else(|| anyhow!("/swap: no `to`"))?,
    )?;
    println!(
        "swap: router={router}  grossOut={}  netOut={}",
        swap["grossOut"], swap["netOut"]
    );

    // Reconstruct the EIP-712 SwapAuthorization struct from the response `auth`.
    let a = &swap["auth"];
    let route_hash_s = a["routeHash"].as_str().ok_or_else(|| anyhow!("no routeHash"))?;
    let auth = SwapAuthorization {
        trader: addr_from_json(a, "trader")?,
        tokenIn: addr_from_json(a, "tokenIn")?,
        tokenOut: addr_from_json(a, "tokenOut")?,
        amountIn: u256_from_json(a, "amountIn")?,
        minOut: u256_from_json(a, "minOut")?,
        feeBps: u64_from_json(a, "feeBps")? as u32,
        feeRecipient: addr_from_json(a, "feeRecipient")?,
        recipient: addr_from_json(a, "recipient")?,
        deadline: u256_from_json(a, "deadline")?,
        nonce: u256_from_json(a, "nonce")?,
        routeHash: FixedBytes::<32>::from_str(route_hash_s).context("bad routeHash")?,
        swapMode: u64_from_json(a, "swapMode")? as u8,
    };

    // ---- 3) EIP-712 sign --------------------------------------------------------
    // domain = { name:"MoonLite", version:"1", chainId, verifyingContract: `to` }.
    let domain = eip712_domain! {
        name: "MoonLite",
        version: "1",
        chain_id: CHAIN_ID,
        verifying_contract: router,
    };
    // Sanity check: our locally computed digest should equal the server's `digest`.
    let digest = auth.eip712_signing_hash(&domain);
    if let Some(server_digest) = swap["digest"].as_str() {
        let ours = format!("0x{:x}", digest);
        if !ours.eq_ignore_ascii_case(server_digest) {
            return Err(anyhow!(
                "EIP-712 digest mismatch: local {ours} != server {server_digest}"
            ));
        }
        println!("eip712 digest OK: {ours}");
    }
    // Sign the 32-byte digest, then emit r||s||v with v in {27,28} (the router expects
    // the classic form; `as_bytes()` would give 0/1, so we build the 65 bytes here).
    let sig: Signature = signer.sign_hash_sync(&digest)?;
    let mut sig65 = Vec::with_capacity(65);
    sig65.extend_from_slice(&sig.r().to_be_bytes::<32>());
    sig65.extend_from_slice(&sig.s().to_be_bytes::<32>());
    sig65.push(27 + sig.v() as u8);
    let signature = Bytes::from(sig65);

    // ---- 4) parse hops for the tx ----------------------------------------------
    let hops = parse_hops(&swap["hops"])?;

    // ---- 5) provider + approve + submit ----------------------------------------
    let wallet = EthereumWallet::from(signer);
    // `connect_http` wants a parsed Url; reqwest::Url is the same `url::Url` alloy
    // uses, so we parse into it explicitly to pin the type.
    let rpc: reqwest::Url = rpc_url.parse().context("bad RPC_URL")?;
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc);

    // Approve the router for the native/base token too — harmless if it's ignored,
    // required for ERC20s. We skip only when allowance already covers amountIn.
    let token_in_addr = auth.tokenIn;
    let erc20 = Erc20::new(token_in_addr, &provider);
    let current = erc20.allowance(trader, router).call().await
        .context("allowance() read failed")?;
    if current < auth.amountIn {
        println!("approving {router} to spend {} of {token_in_addr}...", auth.amountIn);
        let approve_tx = erc20.approve(router, auth.amountIn).send().await
            .context("approve() send failed")?;
        let approve_hash = *approve_tx.watch().await.context("approve() mined")?;
        println!("approve tx: {approve_hash}");
    } else {
        println!("allowance already sufficient ({current}); skipping approve");
    }

    // Submit router.swapExactIn(auth, signature, hops).
    let router_contract = Router::new(router, &provider);
    let pending = router_contract
        .swapExactIn(auth, signature, hops)
        .send()
        .await
        .context("swapExactIn() send failed")?;
    let tx_hash = *pending.tx_hash();
    println!("swap tx submitted: {tx_hash}");
    // Wait for it to be mined so we surface reverts instead of exiting early.
    let mined = pending.watch().await.context("swapExactIn() mined")?;
    println!("swap tx mined: {mined}");

    Ok(())
}

/// Convert the /swap `hops` JSON array into the ABI Hop[] the router expects.
fn parse_hops(v: &serde_json::Value) -> Result<Vec<Hop>> {
    let arr = v.as_array().ok_or_else(|| anyhow!("/swap: hops is not an array"))?;
    let mut hops = Vec::with_capacity(arr.len());
    for h in arr {
        let legs_json = h["legs"].as_array().ok_or_else(|| anyhow!("hop.legs not array"))?;
        let mut legs = Vec::with_capacity(legs_json.len());
        for l in legs_json {
            legs.push(Leg {
                adapter: addr(l["adapter"].as_str().ok_or_else(|| anyhow!("leg.adapter"))?)?,
                amountIn: U256::from_str(
                    l["amountIn"].as_str().ok_or_else(|| anyhow!("leg.amountIn"))?,
                )?,
                data: Bytes::from_str(l["data"].as_str().ok_or_else(|| anyhow!("leg.data"))?)?,
            });
        }
        hops.push(Hop {
            tokenIn: addr(h["tokenIn"].as_str().ok_or_else(|| anyhow!("hop.tokenIn"))?)?,
            tokenOut: addr(h["tokenOut"].as_str().ok_or_else(|| anyhow!("hop.tokenOut"))?)?,
            legs,
        });
    }
    Ok(hops)
}
