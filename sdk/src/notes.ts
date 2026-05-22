// Note model, encryption for recipient discovery, and a local note store.
//
// A note is encrypted to the recipient with an ephemeral X25519 key + a
// XChaCha20-Poly1305 AEAD. Wallets trial-decrypt every note event in a block
// to find incoming notes (Zcash-style). The viewing/spending key here is a
// single field scalar `s`; `pk = Poseidon1(s)` addresses the note.

import { xchacha20poly1305 } from "@noble/ciphers/chacha";
import { x25519 } from "@noble/curves/ed25519";
import { sha256 } from "@noble/hashes/sha256";
import { frToBytes, frFromBytes, FIELD_MODULUS } from "./field.js";
import { noteCommitment, nullifier, ownerPk } from "./poseidon.js";

export interface Note {
  assetId: bigint;
  amount: bigint;
  ownerPk: bigint;
  blinding: bigint;
}

export interface StoredNote extends Note {
  leafIndex: number;
  commitment: bigint;
  spent: boolean;
}

const PLAINTEXT_LEN = 32 * 3 + 8; // assetId, ownerPk, blinding (32 each) + amount (8)

function encodeNotePlaintext(note: Note): Uint8Array {
  const out = new Uint8Array(PLAINTEXT_LEN);
  out.set(frToBytes(note.assetId), 0);
  new DataView(out.buffer).setBigUint64(32, note.amount, true);
  out.set(frToBytes(note.ownerPk), 40);
  out.set(frToBytes(note.blinding), 72);
  return out;
}

function decodeNotePlaintext(bytes: Uint8Array): Note {
  if (bytes.length !== PLAINTEXT_LEN) throw new Error("bad note plaintext");
  const assetId = frFromBytes(bytes.slice(0, 32));
  const amount = new DataView(bytes.buffer, bytes.byteOffset).getBigUint64(32, true);
  const ownerPk = frFromBytes(bytes.slice(40, 72));
  const blinding = frFromBytes(bytes.slice(72, 104));
  return { assetId, amount, ownerPk, blinding };
}

/// Derive an X25519 keypair from the spending key (deterministic) so a
/// wallet only needs `s`. The X25519 secret is `sha256("zul:enc" || s)`.
export function encryptionKeypair(spendingKey: bigint): {
  secret: Uint8Array;
  publicKey: Uint8Array;
} {
  const seed = sha256(
    new Uint8Array([...new TextEncoder().encode("zul:enc"), ...frToBytes(spendingKey)]),
  );
  const secret = seed.slice(0, 32);
  return { secret, publicKey: x25519.getPublicKey(secret) };
}

/// Encrypt a note to `recipientEncPk` (their X25519 public key). Layout:
/// ephemeral_pubkey(32) || nonce(24) || ciphertext(+16 tag).
export function encryptNote(note: Note, recipientEncPk: Uint8Array): Uint8Array {
  const ephemeralSecret = x25519.utils.randomPrivateKey();
  const ephemeralPublic = x25519.getPublicKey(ephemeralSecret);
  const shared = x25519.getSharedSecret(ephemeralSecret, recipientEncPk);
  const key = sha256(shared);
  const nonce = new Uint8Array(24);
  globalThis.crypto.getRandomValues(nonce);
  const cipher = xchacha20poly1305(key, nonce);
  const ciphertext = cipher.encrypt(encodeNotePlaintext(note));
  const out = new Uint8Array(32 + 24 + ciphertext.length);
  out.set(ephemeralPublic, 0);
  out.set(nonce, 32);
  out.set(ciphertext, 56);
  return out;
}

/// Attempt to decrypt; returns null if this note isn't ours.
export function tryDecryptNote(
  encrypted: Uint8Array,
  spendingKey: bigint,
): Note | null {
  if (encrypted.length < 56 + 16) return null;
  try {
    const { secret } = encryptionKeypair(spendingKey);
    const ephemeralPublic = encrypted.slice(0, 32);
    const nonce = encrypted.slice(32, 56);
    const ciphertext = encrypted.slice(56);
    const shared = x25519.getSharedSecret(secret, ephemeralPublic);
    const key = sha256(shared);
    const plaintext = xchacha20poly1305(key, nonce).decrypt(ciphertext);
    return decodeNotePlaintext(plaintext);
  } catch {
    return null;
  }
}

/// In-memory note store. Persist `toJSON()` in the wallet.
export class NoteStore {
  private notes: StoredNote[] = [];

  add(note: StoredNote): void {
    if (!this.notes.some((n) => n.commitment === note.commitment)) {
      this.notes.push(note);
    }
  }

  markSpent(commitment: bigint): void {
    const note = this.notes.find((n) => n.commitment === commitment);
    if (note) note.spent = true;
  }

  unspent(assetId?: bigint): StoredNote[] {
    return this.notes.filter(
      (n) => !n.spent && (assetId === undefined || n.assetId === assetId),
    );
  }

  balance(assetId: bigint): bigint {
    return this.unspent(assetId).reduce((sum, n) => sum + n.amount, 0n);
  }

  toJSON(): string {
    return JSON.stringify(
      this.notes.map((n) => ({
        assetId: n.assetId.toString(),
        amount: n.amount.toString(),
        ownerPk: n.ownerPk.toString(),
        blinding: n.blinding.toString(),
        leafIndex: n.leafIndex,
        commitment: n.commitment.toString(),
        spent: n.spent,
      })),
    );
  }

  static fromJSON(json: string): NoteStore {
    const store = new NoteStore();
    for (const n of JSON.parse(json)) {
      store.add({
        assetId: BigInt(n.assetId),
        amount: BigInt(n.amount),
        ownerPk: BigInt(n.ownerPk),
        blinding: BigInt(n.blinding),
        leafIndex: n.leafIndex,
        commitment: BigInt(n.commitment),
        spent: n.spent,
      });
    }
    return store;
  }
}

/// Scan a note event: if it decrypts to us and the commitment matches the
/// recomputed one, store it. Returns the recovered note or null.
export async function scanNoteEvent(
  store: NoteStore,
  spendingKey: bigint,
  leafIndex: number,
  commitment: bigint,
  encryptedNote: Uint8Array,
): Promise<StoredNote | null> {
  const decrypted = tryDecryptNote(encryptedNote, spendingKey);
  if (!decrypted) return null;

  // Verify the note is really ours: pk must match and the commitment must
  // reproduce. Guards against malformed/garbage ciphertexts that happen to
  // decrypt.
  const pk = await ownerPk(spendingKey);
  if (decrypted.ownerPk !== pk) return null;
  const recomputed = await noteCommitment(
    decrypted.assetId,
    decrypted.amount,
    decrypted.ownerPk,
    decrypted.blinding,
  );
  if (recomputed !== commitment) return null;

  const stored: StoredNote = {
    ...decrypted,
    leafIndex,
    commitment,
    spent: false,
  };
  store.add(stored);
  return stored;
}

export { FIELD_MODULUS, noteCommitment, nullifier, ownerPk };
