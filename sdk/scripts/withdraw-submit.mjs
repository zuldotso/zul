// Submit an L2 bridge-withdraw transaction: burns the withdrawer's L2 balance
// and commits a withdrawal leaf claimable on L1 via zul-l1-claim.
//
//   # native ZUL withdrawal (48-byte data)
//   WITHDRAWER=<keypair.json> RECIPIENT=<L1 pubkey> AMOUNT=<lamports> NONCE=<n> \
//     node sdk/scripts/withdraw-submit.mjs
//   # SPL withdrawal (80-byte data; ASSET = the L1 mint = asset id): burns the
//   # wrapped token and lets zul-l1-claim --asset release the original on L1
//   ASSET=<L1 mint> WITHDRAWER=... RECIPIENT=... AMOUNT=<token units> NONCE=<n> \
//     node sdk/scripts/withdraw-submit.mjs

import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import fs from "node:fs";

const RPC = process.env.ZUL_RPC ?? "http://127.0.0.1:8899";
const conn = new Connection(RPC, "confirmed");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Reserved bridge-withdraw program id: [0x06, 0x21, ...0, 0x01].
const WITHDRAW_PROGRAM = new PublicKey(
  Uint8Array.from([6, 33, ...new Array(29).fill(0), 1]),
);

const withdrawer = Keypair.fromSecretKey(
  Uint8Array.from(JSON.parse(fs.readFileSync(process.env.WITHDRAWER))),
);
const recipient = new PublicKey(process.env.RECIPIENT);
const amount = BigInt(process.env.AMOUNT);
const nonce = BigInt(process.env.NONCE);
// Optional: the L1 mint (asset id) for an SPL withdrawal. Omitted = native ZUL.
const asset = process.env.ASSET ? new PublicKey(process.env.ASSET) : null;

function le64(n) {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(n);
  return b;
}

async function main() {
  console.log(
    `withdraw ${amount}${asset ? " of SPL " + asset.toBase58() : " lamports"} -> L1 ${recipient.toBase58()} (nonce ${nonce})`,
  );
  // native: recipient(32) amount(8) nonce(8); SPL: recipient(32) asset(32) amount(8) nonce(8)
  const data = asset
    ? Buffer.concat([
        Buffer.from(recipient.toBytes()),
        Buffer.from(asset.toBytes()),
        le64(amount),
        le64(nonce),
      ])
    : Buffer.concat([Buffer.from(recipient.toBytes()), le64(amount), le64(nonce)]);
  const ix = new TransactionInstruction({
    programId: WITHDRAW_PROGRAM,
    keys: [{ pubkey: withdrawer.publicKey, isSigner: true, isWritable: true }],
    data,
  });
  const tx = new Transaction().add(ix);
  tx.feePayer = withdrawer.publicKey;
  tx.recentBlockhash = (await conn.getLatestBlockhash()).blockhash;
  tx.sign(withdrawer);
  const sig = await conn.sendRawTransaction(tx.serialize(), { skipPreflight: true });
  console.log("withdraw tx:", sig);
  for (let i = 0; i < 30; i++) {
    await sleep(700);
    const st = (await conn.getSignatureStatuses([sig])).value[0];
    if (st) {
      console.log("status:", st.err ? JSON.stringify(st.err) : "ok");
      break;
    }
  }
  console.log("withdrawer L2 balance now:", await conn.getBalance(withdrawer.publicKey));
}

main().then(() => process.exit(0)).catch((e) => {
  console.error("WITHDRAW FAILED:", e.message);
  process.exit(1);
});
