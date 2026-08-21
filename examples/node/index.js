// MOON.lite router-aggregator — Node sample client (ESM, viem).
//
// End-to-end flow against the MOON.lite API + Arc testnet router:
//   1) GET  /quote               preview amountOut / priceImpact / feeBps
//   2) POST /swap                build the route -> { auth, hops, to, netOut, digest }
//   3) signTypedData(...)        sign the SwapAuthorization (EIP-712) with the user wallet
//   4) ERC20 approve(router)     if current allowance < amountIn
//   5) router.swapExactIn(...)   submit auth + signature + hops to `to`, wait for receipt
//
// The API swap/quote plane is PUBLIC (no auth). We NEVER send our privateKey to the
// server — the server only signs when you (a backend) explicitly pass one. A wallet
// client signs locally, which is what this example does.
//
// Run:  npm i && node index.js
// Env:  API_BASE  RPC_URL  PRIVATE_KEY  [TOKEN_IN] [TOKEN_OUT] [AMOUNT] [RECIPIENT]

import {
  createPublicClient,
  createWalletClient,
  http,
  getAddress,
  formatUnits,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";

// ---------------------------------------------------------------------------
// Config (verified live values; override via env). amounts are integer strings
// in token base units (wei-like, per the token`s decimals).
// ---------------------------------------------------------------------------
const API_BASE = process.env.API_BASE || "http://127.0.0.1:8088";
const RPC_URL = process.env.RPC_URL || "http://127.0.0.1:8545";
const PRIVATE_KEY = process.env.PRIVATE_KEY; // 0x-prefixed 32-byte hex; required
const CHAIN_ID = 5042002; // Arc testnet (0x4cef52)

// BASE / native USDC (gas token) -> a sample ERC20 (JUN). Swap either via env.
const TOKEN_IN = getAddress(
  process.env.TOKEN_IN || "0x3600000000000000000000000000000000000000",
);
const TOKEN_OUT = getAddress(
  process.env.TOKEN_OUT || "0xa4a3f16fc8c1494accd1ae9ebf33cc45bda02275",
);
const AMOUNT = process.env.AMOUNT || "1000000000000000000"; // 1e18 base units

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

  // 1) Quote (public, no auth) — just a preview.
  const quote = await api(
    `/quote?tokenIn=${TOKEN_IN}&tokenOut=${TOKEN_OUT}&amount=${AMOUNT}`,
  );
  console.log("quote:", JSON.stringify(quote, null, 2), "\n");

  // 2) Build the route. For a single swap, input/output are single-element arrays.
  //    We deliberately DO NOT send privateKey — we sign locally below.
  const swap = await api(`/swap`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      inputTokens: [TOKEN_IN],
      outputTokens: [TOKEN_OUT],
      amount: AMOUNT,
      trader: account.address,
      recipient: process.env.RECIPIENT || account.address,
    }),
  });
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
  const signature = await walletClient.signTypedData({
    account,
    domain,
    types: EIP712_TYPES,
    primaryType: "SwapAuthorization",
    message,
  });
  console.log(`signature ${signature}\n`);

  // 4) Approve the router to pull tokenIn, if the current allowance is short.
  //    (The native/base token may not need this, but the check is harmless.)
  const amountIn = BigInt(auth.amountIn);
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

  // 5) Submit. minOut already carries slippage / round-trip protection.
  //    Pass auth as the message object (with bigint uints) the ABI expects.
  const hash = await walletClient.writeContract({
    address: router, abi: ROUTER_ABI, functionName: "swapExactIn",
    args: [message, signature, hops],
  });
  console.log(`swapExactIn tx ${hash}`);
  const receipt = await publicClient.waitForTransactionReceipt({ hash });
  console.log(`mined in block ${receipt.blockNumber}, status ${receipt.status}`);
  console.log(`expected netOut ~ ${formatUnits(BigInt(netOut), 18)} tokenOut`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
