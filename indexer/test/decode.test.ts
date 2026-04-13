import { describe, it, expect } from "vitest";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import { decodeTransaction, base58Encode, POOL_PROGRAM_ID_B58 } from "../src/decode.js";

// Build a real, signed legacy transaction with web3.js and confirm the
// hand-rolled byte parser recovers the fee payer and program names. This
// exercises the actual parsing path (the indexer's other tests use the
// meta.accountKeys fallback).
function serializeBase64(tx: Transaction): string {
  return tx.serialize().toString("base64");
}

describe("decodeTransaction on real serialized transactions", () => {
  const blockhash = base58Encode(new Uint8Array(32).fill(1));

  it("decodes a system transfer: fee payer + program", () => {
    const payer = Keypair.generate();
    const recipient = Keypair.generate();
    const tx = new Transaction();
    tx.recentBlockhash = blockhash;
    tx.feePayer = payer.publicKey;
    tx.add(
      SystemProgram.transfer({
        fromPubkey: payer.publicKey,
        toPubkey: recipient.publicKey,
        lamports: 1000,
      }),
    );
    tx.sign(payer);

    const decoded = decodeTransaction(serializeBase64(tx));
    expect(decoded).not.toBeNull();
    expect(decoded!.feePayer).toBe(payer.publicKey.toBase58());
    expect(decoded!.programs).toContain("system");
    expect(decoded!.touchedPool).toBe(false);
    // All static keys present (payer, recipient, system program).
    expect(decoded!.accountKeys).toContain(payer.publicKey.toBase58());
    expect(decoded!.accountKeys).toContain(recipient.publicKey.toBase58());
  });

  it("flags a transaction that touches the shielded pool program", () => {
    const payer = Keypair.generate();
    const tx = new Transaction();
    tx.recentBlockhash = blockhash;
    tx.feePayer = payer.publicKey;
    tx.add(
      new TransactionInstruction({
        programId: new PublicKey(POOL_PROGRAM_ID_B58),
        keys: [{ pubkey: payer.publicKey, isSigner: true, isWritable: true }],
        data: Buffer.from([0, 0, 0, 0]),
      }),
    );
    tx.sign(payer);

    const decoded = decodeTransaction(serializeBase64(tx));
    expect(decoded).not.toBeNull();
    expect(decoded!.touchedPool).toBe(true);
    expect(decoded!.programs).toContain("shielded-pool");
  });

  it("handles a multi-instruction transaction with many accounts", () => {
    const payer = Keypair.generate();
    const a = Keypair.generate();
    const b = Keypair.generate();
    const tx = new Transaction();
    tx.recentBlockhash = blockhash;
    tx.feePayer = payer.publicKey;
    tx.add(
      SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: a.publicKey, lamports: 1 }),
      SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: b.publicKey, lamports: 2 }),
    );
    tx.sign(payer);

    const decoded = decodeTransaction(serializeBase64(tx));
    expect(decoded).not.toBeNull();
    expect(decoded!.feePayer).toBe(payer.publicKey.toBase58());
    expect(decoded!.accountKeys).toContain(a.publicKey.toBase58());
    expect(decoded!.accountKeys).toContain(b.publicKey.toBase58());
  });

  it("returns null on garbage input", () => {
    expect(decodeTransaction("")).toBeNull();
    expect(decodeTransaction("!!!notbase64!!!")).toBeNull();
  });
});
