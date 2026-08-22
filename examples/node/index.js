// MOON.lite router-aggregator — Node sample client (ESM, viem).
//
// End-to-end flow against the MOON.lite API + Arc testnet router, using the
// fast /submit path for quicker inclusion of the swap:
//   1) GET  /quote               preview amountOut / priceImpact / feeBps
//   2) POST /swap                build the route -> { auth, hops, to, netOut, digest }
//   3) signTypedData(...)        sign the SwapAuthorization (EIP-712) with the user wallet
//   4) ERC20 approve(router)     if current allowance < amountIn (a normal wallet tx)
//   5) sign the swap tx locally  then POST the raw signed tx to /submit for fast inclusion
//
// The API swap/quote/submit plane is PUBLIC (no auth). We NEVER send our privateKey to
// the server. The user signs everything locally and pays their own gas; /submit only
// accepts an already-signed swapExactIn transaction targeting the router.
//
// Run:  npm i && node index.js
// Env:  PRIVATE_KEY  (0x-prefixed 32-byte hex; required — the only secret)

import {
  createPublicClient,
  createWalletClient,
  http,
  getAddress,
  formatUnits,
  encodeFunctionData,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";

// ---------------------------------------------------------------------------
// Config - edit these constants. Only PRIVATE_KEY comes from env (it is a secret).
// in token base units (wei-like, per the token`s decimals).
// ---------------------------------------------------------------------------
const API_BASE = "https://api.moonlite.so";
const RPC_URL = "https://api.moonlite.so/rpc"; // Arc testnet wallet JSON-RPC
const PRIVATE_KEY = process.env.PRIVATE_KEY; // 0x-prefixed 32-byte hex; required
const CHAIN_ID = 5042002; // Arc testnet (0x4cef52)

// BASE / native USDC (gas token) -> a sample ERC20 (JUN). Swap either via env.
const TOKEN_IN = getAddress("0x3600000000000000000000000000000000000000"); // USDC (base / gas token)
const TOKEN_OUT = getAddress("0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275"); // JUN (sample ERC20)
const AMOUNT = "1000000000000000000"; // 1e18 base units of TOKEN_IN
const SLIPPAGE_BPS = 500; // 5% - the router floors auth.minOut by this
const RECIPIENT = null; // null -> defaults to your own address

if (!PRIVATE_KEY) {
  console.error("Set PRIVATE_KEY (0x-prefixed 32-byte hex).");
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Minimal ABIs.
// ---------------------------------------------------------------------------
const ERC20_ABI = [
  { type: "function", name: "allowance", stateMutability: "view",
    inputs: [{ name: "owner", type: "address" }, { name: "spender", type: "address" }],
    outputs: [{ type: "uint256" }] },
  { type: "function", name: "approve", stateMutability: "nonpayable",
    inputs: [{ name: "spender", type: "address" }, { name: "amount", type: "uint256" }],
    outputs: [{ type: "bool" }] },
];

// auth tuple + hops[] tuple exactly as the router expects.
const AUTH_COMPONENTS = [
  { name: "trader", type: "address" },
  { name: "tokenIn", type: "address" },
  { name: "tokenOut", type: "address" },
  { name: "amountIn", type: "uint256" },
  { name: "minOut", type: "uint256" },
  { name: "feeBps", type: "uint32" },
  { name: "feeRecipient", type: "address" },
  { name: "recipient", type: "address" },
  { name: "deadline", type: "uint256" },
  { name: "nonce", type: "uint256" },
  { name: "routeHash", type: "bytes32" },
  { name: "swapMode", type: "uint8" },
];
const HOPS_COMPONENTS = [
  { name: "tokenIn", type: "address" },
  { name: "tokenOut", type: "address" },
  { name: "legs", type: "tuple[]", components: [
    { name: "adapter", type: "address" },
    { name: "amountIn", type: "uint256" },
    { name: "data", type: "bytes" },
  ] },
];
const ROUTER_ABI = [
  { type: "function", name: "swapExactIn", stateMutability: "nonpayable",
    inputs: [
      { name: "auth", type: "tuple", components: AUTH_COMPONENTS },
      { name: "signature", type: "bytes" },
      { name: "hops", type: "tuple[]", components: HOPS_COMPONENTS },
    ],
    outputs: [{ name: "netOut", type: "uint256" }] },
];

// EIP-712 typed-data definition for the SwapAuthorization signature.
const EIP712_TYPES = {
  SwapAuthorization: AUTH_COMPONENTS, // identical field order to the tuple above
};

// ---------------------------------------------------------------------------
// Clients.
// ---------------------------------------------------------------------------
const chain = {
  id: CHAIN_ID,
  name: "Arc testnet",
  nativeCurrency: { name: "USDC", symbol: "USDC", decimals: 18 },
  rpcUrls: { default: { http: [RPC_URL] } },
};
const account = privateKeyToAccount(PRIVATE_KEY);
const publicClient = createPublicClient({ chain, transport: http(RPC_URL) });
const walletClient = createWalletClient({ account, chain, transport: http(RPC_URL) });

async function api(path, init) {
  const res = await fetch(`${API_BASE}${path}`, init);
  if (!res.ok) throw new Error(`${path} -> ${res.status} ${await res.text()}`);
  return res.json();
}

async function main() {
  console.log(`Trader ${account.address} on chain ${CHAIN_ID}`);
  console.log(`Swapping ${AMOUNT} of ${TOKEN_IN} -> ${TOKEN_OUT}\n`);

  // Per-step timers (ms). Filled in as we go; recapped at the end.
  const t = { quote: 0, swap: 0, sign: 0, approve: 0, submit: 0 };
  const t0 = performance.now();

  // 1) Quote (public, no auth) — just a preview.
  let s = performance.now();
  const quote = await api(
    `/quote?tokenIn=${TOKEN_IN}&tokenOut=${TOKEN_OUT}&amount=${AMOUNT}`,
  );
  t.quote = performance.now() - s;
  console.log("quote:", JSON.stringify(quote, null, 2), "\n");

  // 2) Build the route. For a single swap, input/output are single-element arrays.
  //    We deliberately DO NOT send privateKey — we sign locally below.
  s = performance.now();
  const swap = await api(`/swap`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      inputTokens: [TOKEN_IN],
      outputTokens: [TOKEN_OUT],
      amount: AMOUNT,
      trader: account.address,
      recipient: RECIPIENT || account.address,
      slippageBps: SLIPPAGE_BPS,
    }),
  });
  t.swap = performance.now() - s;
  const { auth, hops, to, netOut, grossOut } = swap;
  const router = getAddress(to);
  console.log(`route -> router ${router}`);
  console.log(`grossOut ${grossOut}  netOut ${netOut}  (minOut ${auth.minOut})\n`);

  // 3) Sign the SwapAuthorization via EIP-712 with the user`s wallet.
  //    verifyingContract MUST be the `to` the API returned.
  const domain = {
    name: "MoonLite",
    version: "1",
    chainId: CHAIN_ID,
    verifyingContract: router,
  };
  // viem wants bigint for uint fields; the API returns them as decimal strings.
  const message = {
    ...auth,
    amountIn: BigInt(auth.amountIn),
    minOut: BigInt(auth.minOut),
    deadline: BigInt(auth.deadline),
    nonce: BigInt(auth.nonce),
    feeBps: Number(auth.feeBps),
    swapMode: Number(auth.swapMode),
  };
  s = performance.now();
  const signature = await walletClient.signTypedData({
    account,
    domain,
    types: EIP712_TYPES,
    primaryType: "SwapAuthorization",
    message,
  });
  t.sign = performance.now() - s;
  console.log(`signature ${signature}\n`);

  // 4) Approve the router to pull tokenIn, if the current allowance is short.
  //    approve is a plain wallet transaction (NOT a swap): the wallet sends it
  //    directly and we wait for it to confirm before submitting the swap.
  const amountIn = BigInt(auth.amountIn);
  s = performance.now();
  const allowance = await publicClient.readContract({
    address: TOKEN_IN, abi: ERC20_ABI, functionName: "allowance",
    args: [account.address, router],
  });
  if (allowance < amountIn) {
    console.log(`allowance ${allowance} < ${amountIn}; approving...`);
    const approveHash = await walletClient.writeContract({
      address: TOKEN_IN, abi: ERC20_ABI, functionName: "approve",
      args: [router, amountIn],
    });
    await publicClient.waitForTransactionReceipt({ hash: approveHash });
    console.log(`approved in ${approveHash}\n`);
  } else {
    console.log(`allowance ${allowance} already covers ${amountIn}\n`);
  }
  t.approve = performance.now() - s;

  // 5) Build + sign the swapExactIn transaction locally (user's key, user's gas,
  //    user's nonce), then POST the raw signed tx to /submit for faster inclusion.
  //    minOut carries your slippageBps floor (or round-trip protection).
  s = performance.now();

  // Encode the swapExactIn calldata targeting the router.
  const data = encodeFunctionData({
    abi: ROUTER_ABI,
    functionName: "swapExactIn",
    args: [message, signature, hops],
  });

  // The user pays their own gas: fetch their next nonce and current fees, and
  // estimate the gas the swap will consume.
  const [nonce, fees, gas] = await Promise.all([
    publicClient.getTransactionCount({ address: account.address }),
    publicClient.estimateFeesPerGas(),
    publicClient.estimateGas({ account, to: router, data }),
  ]);

  // Sign the transaction offline — the wallet does not send it; /submit does.
  const signedTx = await walletClient.signTransaction({
    to: router,
    data,
    gas,
    nonce,
    maxFeePerGas: fees.maxFeePerGas,
    maxPriorityFeePerGas: fees.maxPriorityFeePerGas,
    chain,
    type: "eip1559",
  });

  // Hand the raw signed swap tx to /submit for fast inclusion; it returns the hash.
  const submitRes = await api(`/submit`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ rawTx: signedTx }),
  });
  t.submit = performance.now() - s;

  if (!submitRes.ok) throw new Error(`/submit did not accept the tx: ${JSON.stringify(submitRes)}`);
  const hash = submitRes.txHash;
  console.log(`swapExactIn submitted -> ${hash}`);

  const receipt = await publicClient.waitForTransactionReceipt({ hash });
  console.log(`mined in block ${receipt.blockNumber}, status ${receipt.status}`);
  console.log(`expected netOut ~ ${formatUnits(BigInt(netOut), 18)} tokenOut\n`);

  // Timing recap.
  const total = performance.now() - t0;
  const ms = (n) => `${n.toFixed(1)} ms`;
  console.log("=== timing ===");
  console.log(`  quote    :  ${ms(t.quote)}`);
  console.log(`  /swap    :  ${ms(t.swap)}`);
  console.log(`  sign     :  ${ms(t.sign)}`);
  console.log(`  approve  :  ${ms(t.approve)}`);
  console.log(`  /submit  :  ${ms(t.submit)}`);
  console.log(`  total    :  ${ms(total)}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
