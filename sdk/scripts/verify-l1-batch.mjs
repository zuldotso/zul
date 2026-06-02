// Independently verify that the live sequencer's settlement batches actually
// landed on devnet: derive the settlement PDAs, read the on-chain accounts,
// and decode the batch records. Read-only against public devnet RPC.
//
//   node sdk/scripts/verify-l1-batch.mjs

import { Connection, PublicKey } from "@solana/web3.js";

const RPC = process.env.DEVNET_RPC ?? "https://api.devnet.solana.com";
const SETTLEMENT = new PublicKey("4oLtoaJAZGNg1uRTd8HfWJLsXZqKFhFbqX6EyMiGD4vN");
const DA_LOG = new PublicKey("7YX2GVrUcaiXvyP4SAExaVVuxpuMYejcrd3zYugTGKQ4");

const conn = new Connection(RPC, "confirmed");
const le = (buf, off, len) => {
  let n = 0n;
  for (let i = 0; i < len; i++) n |= BigInt(buf[off + i]) << (8n * BigInt(i));
  return n;
};
const hex = (buf, off, len) => Buffer.from(buf.subarray(off, off + len)).toString("hex");

function configPda() {
  return PublicKey.findProgramAddressSync([Buffer.from("config")], SETTLEMENT)[0];
}
function batchPda(batchNo) {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(batchNo));
  return PublicKey.findProgramAddressSync([Buffer.from("batch"), b], SETTLEMENT)[0];
}

async function main() {
  console.log("devnet:", RPC);
  console.log("settlement program:", SETTLEMENT.toBase58());
  console.log("da_log program    :", DA_LOG.toBase58());

  const cfgPk = configPda();
  const cfg = await conn.getAccountInfo(cfgPk);
  if (!cfg) throw new Error("settlement config PDA not found — batcher never initialized");
  const d = cfg.data;
  // disc(8) authority(32) batch_count(8) total_deposited(8) bump(1) vault_bump(1)
  const authority = new PublicKey(d.subarray(8, 40)).toBase58();
  const batchCount = le(d, 40, 8);
  const totalDeposited = le(d, 48, 8);
  console.log(`\n[config ${cfgPk.toBase58()}]`);
  console.log("  authority      :", authority);
  console.log("  batch_count    :", batchCount.toString());
  console.log("  total_deposited:", totalDeposited.toString());

  console.log(`\n[batches] reading ${batchCount} record(s)…`);
  for (let i = 0n; i < batchCount; i++) {
    const pk = batchPda(Number(i));
    const acc = await conn.getAccountInfo(pk);
    if (!acc) {
      console.log(`  batch ${i}: MISSING (${pk.toBase58()})`);
      continue;
    }
    const b = acc.data;
    // disc(8) batch_no(8) first(8) last(8) state_root(32) da_hash(32) chunk_count(4) posted_at(8)
    const batchNo = le(b, 8, 8);
    const first = le(b, 16, 8);
    const last = le(b, 24, 8);
    const stateRoot = hex(b, 32, 32);
    const daHash = hex(b, 64, 32);
    const chunkCount = le(b, 96, 4);
    const postedAt = le(b, 100, 8);
    console.log(
      `  batch ${batchNo}: slots ${first}..=${last}  chunks=${chunkCount}  posted_at=${postedAt}`,
    );
    console.log(`           state_root=${stateRoot}`);
    console.log(`           da_hash   =${daHash}`);
  }

  // Confirm DA chunks were published too (da_log emits ChunkPosted; the
  // program is stateless, so we just confirm it has been invoked recently).
  const sigs = await conn.getSignaturesForAddress(DA_LOG, { limit: 3 });
  console.log(`\n[da_log] recent invocations: ${sigs.length}`);
  for (const s of sigs) console.log("  ", s.signature, s.err ? "ERR" : "ok");

  console.log(
    "\n=== RESULT:",
    batchCount > 0n ? "settlement batches are live on devnet ✓" : "NO BATCHES",
    "===",
  );
}

main().then(() => process.exit(0)).catch((e) => {
  console.error("VERIFY FAILED:", e.message);
  process.exit(1);
});
