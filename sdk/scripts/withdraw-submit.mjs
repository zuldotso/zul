// Submit an L2 bridge-withdraw transaction: burns the withdrawer's L2 ZUL and
// commits a withdrawal leaf claimable on L1 via zul-l1-claim.
//
//   WITHDRAWER=<keypair.json> RECIPIENT=<L1 pubkey> AMOUNT=<lamports> NONCE=<n> \
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

function le64(n) {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(n);
  return b;
}

async function main() {
  console.log(`withdraw ${amount} -> L1 ${recipient.toBase58()} (nonce ${nonce})`);
  const data = Buffer.concat([Buffer.from(recipient.toBytes()), le64(amount), le64(nonce)]);
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
