// Verify WebSocket subscriptions against the live node. web3.js derives the
// WS endpoint as the HTTP port + 1 (8899 -> 8900), so this exercises the real
// pub/sub path: slotSubscribe, accountSubscribe, and signatureSubscribe (the
// last via confirmTransaction).
//
//   node sdk/scripts/ws-check.mjs

import { Connection, Keypair, LAMPORTS_PER_SOL } from "@solana/web3.js";

const RPC = process.env.ZUL_RPC ?? "http://127.0.0.1:8899";
const conn = new Connection(RPC, "confirmed");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  console.log("WS check ->", RPC, "(WebSocket on port + 1)");

  let slotSeen = null;
  const slotSub = conn.onSlotChange((info) => {
    if (slotSeen === null) {
      slotSeen = info.slot;
      console.log("  slotSubscribe fired: slot", info.slot);
    }
  });

  const target = Keypair.generate().publicKey;
  let accountSeen = null;
  const accSub = conn.onAccountChange(target, (info) => {
    if (accountSeen === null) {
      accountSeen = info.lamports;
      console.log("  accountSubscribe fired: lamports", info.lamports);
    }
  });

  // Give the subscriptions a moment to register over the socket.
  await sleep(800);

  console.log("requesting airdrop (produces a block)…");
  const sig = await conn.requestAirdrop(target, 1 * LAMPORTS_PER_SOL);
  const bh = await conn.getLatestBlockhash();
  console.log("confirming via signatureSubscribe (WS push)…");
  const res = await conn.confirmTransaction(
    { signature: sig, blockhash: bh.blockhash, lastValidBlockHeight: bh.lastValidBlockHeight },
    "confirmed",
  );
  console.log("  confirmTransaction resolved: err =", JSON.stringify(res.value.err));

  // Let slot/account notifications arrive if they haven't yet.
  for (let i = 0; i < 20 && (slotSeen === null || accountSeen === null); i++) await sleep(500);

  await conn.removeSlotChangeListener(slotSub);
  await conn.removeAccountChangeListener(accSub);

  const sigOk = res.value && res.value.err === null;
  console.log("\n=== RESULT ===");
  console.log("signatureSubscribe (confirmTransaction):", sigOk ? "PASS" : "CHECK");
  console.log("slotSubscribe   :", slotSeen !== null ? `PASS (slot ${slotSeen})` : "NO NOTIFICATION");
  console.log("accountSubscribe:", accountSeen !== null ? `PASS (${accountSeen} lamports)` : "NO NOTIFICATION");
  const pass = sigOk && slotSeen !== null && accountSeen !== null;
  console.log(pass ? "\nWebSocket pub/sub: ALL PASS ✓" : "\nWebSocket pub/sub: INCOMPLETE");
}

main().then(() => process.exit(0)).catch((e) => {
  console.error("WS CHECK FAILED:", e.message);
  process.exit(1);
});
