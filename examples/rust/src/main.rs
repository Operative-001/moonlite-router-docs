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
//!   5) Build the swapExactIn(auth, signature, hops) transaction, sign it locally
//!      with our own key/gas/nonce, and hand the raw signed tx to /submit for the
//!      fast path (faster inclusion). Print the returned transaction hash.
//!      `minOut` already carries slippage / round-trip protection computed by the
//!      API, so we pass the auth through verbatim.
//!
//! Config via env vars:
//!   PRIVATE_KEY  0x-prefixed 32-byte hex key of the trader wallet (required)
//! Everything else is a non-secret constant at the top of the file.

use alloy::consensus::TxEip1559;
use alloy::eips::eip2718::Encodable2718;
use alloy::network::TxSigner;
use alloy::primitives::{Address, Bytes, FixedBytes, Signature, TxKind, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::SignerSync;
use alloy::sol;
use alloy::sol_types::{eip712_domain, SolCall, SolStruct};
use anyhow::{anyhow, Context, Result};
use std::env;
use std::str::FromStr;
use std::time::Instant;

// Arc testnet.
const CHAIN_ID: u64 = 5042002;
// Native USDC / gas token — the default tokenIn.

// -- Solidity types, shared by EIP-712 signing AND by ABI-encoding the tx. --------
//
// `SwapAuthorization` MUST match the EIP-712 type list exactly (name/type/order);
// the sol! macro derives the correct EIP-712 typeHash from this definition. The same
// struct is reused as the first argument of swapExactIn, and Leg/Hop mirror the
// `hops` array returned by /swap. `MoonRouter::swapExactInCall{...}.abi_encode()`
// produces the exact calldata we sign into the transaction.
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
    contract MoonRouter {
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

// -- small parse helpers ----------------------------------------------------------
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

// Non-secret config - edit these. Only PRIVATE_KEY comes from the environment
// (it is a secret and must never live in source).
const API_BASE: &str = "https://api.moonlite.so";      // MOON.lite API base
const RPC_URL: &str = "https://api.moonlite.so/rpc";   // Arc testnet wallet JSON-RPC
const TOKEN_IN: &str = "0x3600000000000000000000000000000000000000";  // USDC (base / gas token)
const TOKEN_OUT: &str = "0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275"; // JUN (sample ERC20)
const AMOUNT_IN: &str = "1000000000000000000";         // 1e18 base units of TOKEN_IN
const SLIPPAGE_BPS: u32 = 500;                         // 5% - router floors auth.minOut by this

#[tokio::main]
async fn main() -> Result<()> {
    // ---- config -----------------------------------------------------------------
    let api_base = API_BASE.to_string();
    let rpc_url = RPC_URL.to_string();
    let pk = env::var("PRIVATE_KEY").context("PRIVATE_KEY env var is required")?; // SECRET -> env only
    let token_in = TOKEN_IN.to_string();
    let token_out = TOKEN_OUT.to_string();
    let amount_in = AMOUNT_IN.to_string();

    let signer: PrivateKeySigner = pk.trim().parse().context("invalid PRIVATE_KEY")?;
    let trader = signer.address();
    println!("trader (signer) = {trader}");

    let http = reqwest::Client::new();

    // Timing accumulators — each numbered step is wrapped in its own timer and the
    // durations are printed as a recap block at the very end.
    let t_total = Instant::now();

    // ---- 1) quote preview -------------------------------------------------------
    let t_quote = Instant::now();
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
    let ms_quote = t_quote.elapsed().as_millis();
    println!(
        "quote: {} {} -> {} {}  (priceImpactBps={}, feeBps={})",
        quote["amountIn"], token_in, quote["amountOut"], token_out,
        quote["priceImpactBps"], quote["feeBps"],
    );

    // ---- 2) build the swap ------------------------------------------------------
    // Single swap: inputTokens=[tokenIn], outputTokens=[tokenOut]. NO privateKey.
    let t_swap = Instant::now();
    let swap_req = serde_json::json!({
        "inputTokens":  [token_in],
        "outputTokens": [token_out],
        "amount":       amount_in,
        "trader":       trader.to_string(),
        "recipient":    trader.to_string(),
        "slippageBps":  SLIPPAGE_BPS,
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
    let ms_swap = t_swap.elapsed().as_millis();

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

    // Parse hops now so signing (step 3) and calldata (step 5) both have them.
    let hops = parse_hops(&swap["hops"])?;

    // ---- 3) EIP-712 sign the auth ----------------------------------------------
    // domain = { name:"MoonLite", version:"1", chainId, verifyingContract: `to` }.
    let t_sign = Instant::now();
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
    let ms_sign = t_sign.elapsed().as_millis();

    // ---- provider (reads + approve + tx params) --------------------------------
    // `connect_http` wants a parsed Url; reqwest::Url is the same `url::Url` alloy
    // uses, so we parse into it explicitly to pin the type.
    let rpc: reqwest::Url = rpc_url.parse().context("bad RPC_URL")?;
    let provider = ProviderBuilder::new().connect_http(rpc);

    // ---- 4) approve if allowance is short --------------------------------------
    // approve is NOT a swap, so we send it directly via the wallet RPC, not
    // /submit. We skip only when the allowance already covers amountIn.
    let t_approve = Instant::now();
    let token_in_addr = auth.tokenIn;
    let erc20 = Erc20::new(token_in_addr, &provider);
    let current = erc20.allowance(trader, router).call().await
        .context("allowance() read failed")?;
    if current < auth.amountIn {
        println!("approving {router} to spend {} of {token_in_addr}...", auth.amountIn);
        // Build + sign + send the approve tx with the same local signer.
        let approve_calldata = Erc20::approveCall {
            spender: router,
            amount: auth.amountIn,
        }
        .abi_encode();
        let approve_hash = build_sign_send_local(
            &provider,
            &signer,
            token_in_addr,
            Bytes::from(approve_calldata),
        )
        .await
        .context("approve() send failed")?;
        println!("approve tx: {approve_hash}");
        // Wait for the approve to land so the swap doesn't race an un-mined allowance.
        wait_for_receipt(&provider, approve_hash).await?;
    } else {
        println!("allowance already sufficient ({current}); skipping approve");
    }
    let ms_approve = t_approve.elapsed().as_millis();

    // ---- 5) build + sign swapExactIn tx locally, POST raw tx to /submit --------
    // Fast path: we sign the swap with our own key/gas/nonce and hand the raw
    // signed transaction to /submit for faster inclusion. We never server-sign.
    let t_submit = Instant::now();

    // swapExactIn(auth, signature, hops) calldata via the sol! binding.
    let calldata = MoonRouter::swapExactInCall {
        auth,
        signature,
        hops,
    }
    .abi_encode();

    // Gather nonce + fees from the wallet RPC, then build a fully-populated
    // EIP-1559 transaction targeting the router.
    let nonce = provider
        .get_transaction_count(trader)
        .await
        .context("get nonce failed")?;
    let fees = provider.estimate_eip1559_fees().await.context("fee estimate failed")?;
    let gas_limit = provider
        .estimate_gas(
            alloy::rpc::types::TransactionRequest::default()
                .from(trader)
                .to(router)
                .input(Bytes::from(calldata.clone()).into()),
        )
        .await
        .context("gas estimate failed")?;

    let mut tx = TxEip1559 {
        chain_id: CHAIN_ID,
        nonce,
        gas_limit,
        max_fee_per_gas: fees.max_fee_per_gas,
        max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
        to: TxKind::Call(router),
        value: U256::ZERO,
        access_list: Default::default(),
        input: Bytes::from(calldata),
    };

    // Sign the transaction locally with the user's key, then serialize it to the
    // EIP-2718 typed-envelope raw bytes and hex-encode for the wire.
    let tx_sig = signer
        .sign_transaction(&mut tx)
        .await
        .context("sign swap tx failed")?;
    let signed = tx.into_signed(tx_sig);
    let raw = signed.encoded_2718();
    let raw_hex = format!("0x{}", alloy::hex::encode(&raw));

    // Hand the raw signed tx to /submit (fast path).
    let submit_resp: serde_json::Value = http
        .post(format!("{api_base}/submit"))
        .json(&serde_json::json!({ "rawTx": raw_hex }))
        .send()
        .await?
        .error_for_status()
        .context("POST /submit failed")?
        .json()
        .await?;
    let ms_submit = t_submit.elapsed().as_millis();

    if submit_resp["ok"].as_bool() != Some(true) {
        return Err(anyhow!("/submit did not return ok:true -> {submit_resp}"));
    }
    let tx_hash = submit_resp["txHash"]
        .as_str()
        .ok_or_else(|| anyhow!("/submit: no txHash"))?;
    println!("swap submitted (fast path): {tx_hash}");

    let ms_total = t_total.elapsed().as_millis();

    // ---- timing recap -----------------------------------------------------------
    println!();
    println!("=== timing ===");
    println!("  quote    : {ms_quote:>5} ms");
    println!("  /swap    : {ms_swap:>5} ms");
    println!("  sign     : {ms_sign:>5} ms");
    println!("  approve  : {ms_approve:>5} ms");
    println!("  /submit  : {ms_submit:>5} ms");
    println!("  total    : {ms_total:>5} ms");

    Ok(())
}

/// Build, locally sign, and send a simple EIP-1559 call transaction directly via
/// the wallet RPC. Used for non-swap txs (e.g. ERC20 approve). Returns
/// the transaction hash.
async fn build_sign_send_local(
    provider: &impl Provider,
    signer: &PrivateKeySigner,
    to: Address,
    input: Bytes,
) -> Result<FixedBytes<32>> {
    let from = signer.address();
    let nonce = provider.get_transaction_count(from).await?;
    let fees = provider.estimate_eip1559_fees().await?;
    let gas_limit = provider
        .estimate_gas(
            alloy::rpc::types::TransactionRequest::default()
                .from(from)
                .to(to)
                .input(input.clone().into()),
        )
        .await?;

    let mut tx = TxEip1559 {
        chain_id: CHAIN_ID,
        nonce,
        gas_limit,
        max_fee_per_gas: fees.max_fee_per_gas,
        max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
        to: TxKind::Call(to),
        value: U256::ZERO,
        access_list: Default::default(),
        input,
    };
    let sig = signer.sign_transaction(&mut tx).await?;
    let signed = tx.into_signed(sig);
    let raw = signed.encoded_2718();
    let pending = provider.send_raw_transaction(&raw).await?;
    Ok(*pending.tx_hash())
}

/// Poll for a transaction receipt so callers can be sure a tx has landed.
async fn wait_for_receipt(provider: &impl Provider, hash: FixedBytes<32>) -> Result<()> {
    loop {
        if let Some(receipt) = provider.get_transaction_receipt(hash).await? {
            if !receipt.status() {
                return Err(anyhow!("tx {hash} reverted"));
            }
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
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
