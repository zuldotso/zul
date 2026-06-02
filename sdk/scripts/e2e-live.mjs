// Tier-2 end-to-end against the LIVE node: shield -> private transfer ->
// unshield, with real Groth16 proofs generated here by snarkjs. Proves the
// privacy layer works on the running chain.
//
//   node sdk/scripts/e2e-live.mjs            # against the OVH node
//   ZUL_RPC=http://127.0.0.1:8899 node ...   # against a local node
//
// Self-contained: this script performs every pool op, so it knows each
// note's leaf index and mirrors the commitment tree locally (the pool
// starts empty on a fresh chain).

import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import {
  POOL_PROGRAM_ID,
  buildShield,
  buildTransfer,
  buildUnshield,
  MerkleTree,
  ownerPk,
  encryptionKeypair,
  randomFr,
  NATIVE_ASSET_ID,
} from "../dist/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const CIRCUITS = join(here, "..", "..", "circuits", "build");
const transferArtifacts = {
  wasm: join(CIRCUITS, "transfer", "transfer_js", "transfer.wasm"),
  zkey: join(CIRCUITS, "transfer", "transfer_final.zkey"),
};
const unshieldArtifacts = {
  wasm: join(CIRCUITS, "unshield", "unshield_js", "unshield.wasm"),
  zkey: join(CIRCUITS, "unshield", "unshield_final.zkey"),
};

const RPC = process.env.ZUL_RPC ?? "http://127.0.0.1:8899";
const ZUL = 1_000_000_000n;
const POOL_STATE = new PublicKey(
  Uint8Array.from([5, 0x21, ...new Array(29).fill(0), 1]),
);
const POOL_VAULT = new PublicKey(
  Uint8Array.from([5, 0x21, ...new Array(29).fill(0), 2]),
);

const conn = new Connection(RPC, "confirmed");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function balance(pk) {
  return BigInt(await conn.getBalance(new PublicKey(pk)));
}

// Submit a single-instruction pool transaction and wait until the node has
// processed it (its signature becomes queryable).
async function sendPool(payer, data, label) {
  const ix = new TransactionInstruction({
    programId: new PublicKey(POOL_PROGRAM_ID),
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: POOL_STATE, isSigner: false, isWritable: true },
      { pubkey: POOL_VAULT, isSigner: false, isWritable: true },
    ],
    data: Buffer.from(data),
  });
  const tx = new Transaction().add(ix);
  tx.feePayer = payer.publicKey;
  tx.recentBlockhash = (await conn.getLatestBlockhash()).blockhash;
  tx.sign(payer);
  const sig = await conn.sendRawTransaction(tx.serialize(), {
    skipPreflight: true,
  });
  // Wait for the node to include + process it.
  for (let i = 0; i < 40; i++) {
    await sleep(700);
    const st = await conn.getSignatureStatuses([sig]);
    const s = st.value[0];
    if (s) {
      if (s.err) throw new Error(`${label} failed on-chain: ${JSON.stringify(s.err)}`);
      return sig;
    }
  }
  throw new Error(`${label}: not processed in time (${sig})`);
}

function toStoredNote(note, leafIndex, commitment) {
  return { ...note, leafIndex, commitment, spent: false };
}

async function main() {
  console.log(`ZUL privacy e2e -> ${RPC}`);
  const version = await conn.getVersion();
  console.log("node:", JSON.stringify(version));

  // --- identities ---
  const payer = Keypair.generate();
  const s = randomFr(); // spending key (one identity, self-transfer)
  const pk = await ownerPk(s);
  const enc = encryptionKeypair(s);
  const tree = new MerkleTree();

  // --- fund the payer from the faucet ---
  console.log("\n[1] airdrop 9 ZUL to payer", payer.publicKey.toBase58());
  const air = await conn.requestAirdrop(payer.publicKey, Number(9n * ZUL));
  for (let i = 0; i < 40; i++) {
    await sleep(700);
    if ((await conn.getSignatureStatuses([air])).value[0]) break;
  }
  console.log("    payer balance:", (await balance(payer.publicKey)) / ZUL, "ZUL");

  const vault0 = await balance(POOL_VAULT);

  // --- [2] SHIELD 5 ZUL ---
  console.log("\n[2] shield 5 ZUL into the pool");
  const shield = await buildShield({
    asset: { kind: "native" },
    assetId: NATIVE_ASSET_ID,
    amount: 5n * ZUL,
    recipientPk: pk,
    recipientEncPk: enc.publicKey,
  });
  await sendPool(payer, shield.data, "shield");
  const note0 = toStoredNote(shield.note, 0, shield.commitment);
  await tree.append(shield.commitment); // leaf 0
  console.log("    note0 committed at leafIndex 0");
  console.log("    vault +", ((await balance(POOL_VAULT)) - vault0) / ZUL, "ZUL");

  // --- [3] PRIVATE TRANSFER: 5 -> 3 + 2 (self), real proof ---
  console.log("\n[3] private transfer (2-in/2-out, Groth16 proof)…");
  const dummy = toStoredNote(
    { assetId: NATIVE_ASSET_ID, amount: 0n, ownerPk: pk, blinding: randomFr() },
    0,
    0n,
  );
  const transfer = await buildTransfer({
    spendingKey: s,
    assetId: NATIVE_ASSET_ID,
    inputs: [note0, dummy],
    outputs: [
      { amount: 3n * ZUL, recipientPk: pk, recipientEncPk: enc.publicKey },
      { amount: 2n * ZUL, recipientPk: pk, recipientEncPk: enc.publicKey },
    ],
    fee: 0n,
    tree,
    artifacts: transferArtifacts,
  });
  console.log("    proof generated, submitting…");
  await sendPool(payer, transfer.data, "transfer");
  // output notes land at leaves 1 and 2
  const outA = toStoredNote(transfer.outputNotes[0], 1, await commitmentOf(transfer.outputNotes[0]));
  await tree.append(outA.commitment);
  const outB = toStoredNote(transfer.outputNotes[1], 2, await commitmentOf(transfer.outputNotes[1]));
  await tree.append(outB.commitment);
  console.log("    transfer accepted; 2 new notes at leaves 1,2");

  // double-spend check: replaying the same transfer must be rejected
  console.log("    re-submitting same transfer (expect rejection)…");
  let doubleSpendRejected = false;
  try {
    await sendPool(payer, transfer.data, "transfer-replay");
  } catch {
    doubleSpendRejected = true;
  }
  console.log("    double-spend rejected:", doubleSpendRejected);

  // --- [4] UNSHIELD 3 ZUL -> public recipient ---
  console.log("\n[4] unshield 3 ZUL to a public recipient…");
  const recipient = Keypair.generate().publicKey;
  const rec0 = await balance(recipient);
  const unshield = await buildUnshield({
    spendingKey: s,
    asset: { kind: "native" },
    assetId: NATIVE_ASSET_ID,
    input: outA, // 3 ZUL note
    amount: 3n * ZUL,
    recipient: recipient.toBytes(),
    changePk: pk,
    changeEncPk: enc.publicKey,
    tree,
    artifacts: unshieldArtifacts,
  });
  console.log("    proof generated, submitting…");
  await sendPool(payer, unshield.data, "unshield");
  const recDelta = (await balance(recipient)) - rec0;
  console.log("    recipient", recipient.toBase58(), "received", recDelta / ZUL, "ZUL");

  console.log("\n=== RESULT ===");
  console.log("shield 5 ZUL -> private transfer 5=3+2 -> unshield 3 ZUL:",
    recDelta === 3n * ZUL && doubleSpendRejected ? "PASS ✓" : "MISMATCH");
}

// recompute a note's commitment (output notes from buildTransfer carry the
// fields; commitment derivation matches the chain).
async function commitmentOf(note) {
  const { noteCommitment } = await import("../dist/index.js");
  return noteCommitment(note.assetId, note.amount, note.ownerPk, note.blinding);
}

main().then(() => process.exit(0)).catch((e) => {
  console.error("\nE2E FAILED:", e.message);
  process.exit(1);
});
