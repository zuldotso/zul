import { describe, it, expect } from "vitest";
import { frToBytes, frFromBytes, frToBytesBE, FIELD_MODULUS } from "../src/field.js";
import {
  ownerPk,
  noteCommitment,
  nullifier,
  assetIdForMint,
  NATIVE_ASSET_ID,
} from "../src/poseidon.js";
import {
  NoteStore,
  encryptNote,
  tryDecryptNote,
  encryptionKeypair,
  scanNoteEvent,
  type Note,
} from "../src/notes.js";
import { MerkleTree } from "../src/merkle.js";

describe("field encoding", () => {
  it("round-trips little-endian", () => {
    for (const v of [0n, 1n, 255n, 256n, 1_000_000n, FIELD_MODULUS - 1n]) {
      expect(frFromBytes(frToBytes(v))).toBe(v);
    }
  });

  it("rejects non-canonical bytes", () => {
    expect(() => frFromBytes(new Uint8Array(32).fill(0xff))).toThrow();
  });

  it("big-endian is the reverse of little-endian", () => {
    const le = frToBytes(0x0102n);
    const be = frToBytesBE(0x0102n);
    expect(Array.from(be)).toEqual(Array.from(le).reverse());
  });
});

describe("poseidon scheme", () => {
  it("derives stable, distinct hashes", async () => {
    const pk = await ownerPk(123n);
    const cm = await noteCommitment(NATIVE_ASSET_ID, 100n, pk, 7n);
    const cm2 = await noteCommitment(NATIVE_ASSET_ID, 101n, pk, 7n);
    expect(cm).not.toBe(cm2);
    const nf = await nullifier(cm, 0n, 123n);
    const nf2 = await nullifier(cm, 1n, 123n);
    expect(nf).not.toBe(nf2);
    // Determinism.
    expect(await ownerPk(123n)).toBe(pk);
  });

  it("asset ids: native is zero, mint reduces into the field", () => {
    expect(NATIVE_ASSET_ID).toBe(0n);
    const id = assetIdForMint(new Uint8Array(32).fill(1));
    expect(id).toBeGreaterThan(0n);
    expect(id).toBeLessThan(FIELD_MODULUS);
  });
});

describe("note encryption + discovery", () => {
  it("encrypts to a recipient and only they can decrypt", () => {
    const aliceKey = 42n;
    const { publicKey: alicePk } = encryptionKeypair(aliceKey);
    const note: Note = { assetId: 0n, amount: 500n, ownerPk: 99n, blinding: 7n };

    const encrypted = encryptNote(note, alicePk);
    const recovered = tryDecryptNote(encrypted, aliceKey);
    expect(recovered).not.toBeNull();
    expect(recovered!.amount).toBe(500n);

    // A different key cannot decrypt.
    expect(tryDecryptNote(encrypted, 43n)).toBeNull();
  });

  it("scanNoteEvent verifies pk and commitment before storing", async () => {
    const spendingKey = 77n;
    const pk = await ownerPk(spendingKey);
    const { publicKey: encPk } = encryptionKeypair(spendingKey);
    const note: Note = { assetId: 0n, amount: 1000n, ownerPk: pk, blinding: 5n };
    const commitment = await noteCommitment(0n, 1000n, pk, 5n);
    const encrypted = encryptNote(note, encPk);

    const store = new NoteStore();
    const stored = await scanNoteEvent(store, spendingKey, 3, commitment, encrypted);
    expect(stored).not.toBeNull();
    expect(store.balance(0n)).toBe(1000n);

    // Wrong commitment is rejected even if decryption succeeds.
    const store2 = new NoteStore();
    const bad = await scanNoteEvent(store2, spendingKey, 3, 999n, encrypted);
    expect(bad).toBeNull();
  });

  it("note store persists and reloads", () => {
    const store = new NoteStore();
    store.add({
      assetId: 0n,
      amount: 100n,
      ownerPk: 1n,
      blinding: 2n,
      leafIndex: 0,
      commitment: 42n,
      spent: false,
    });
    const reloaded = NoteStore.fromJSON(store.toJSON());
    expect(reloaded.balance(0n)).toBe(100n);
    reloaded.markSpent(42n);
    expect(reloaded.balance(0n)).toBe(0n);
  });
});

describe("merkle tree mirror", () => {
  it("matches root growth and yields verifiable paths", async () => {
    const tree = new MerkleTree();
    const i0 = await tree.append(111n);
    const i1 = await tree.append(222n);
    expect(i0).toBe(0);
    expect(i1).toBe(1);
    const path = await tree.path(0);
    expect(path.length).toBe(26);
    const root = await tree.root();
    expect(root).toBeGreaterThan(0n);
  });
});
