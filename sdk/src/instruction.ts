// PoolInstruction encoding, byte-compatible with the Rust serde/bincode
// representation in chain/zul-privacy/src/instruction.rs.
//
// Enum layout (bincode v1): u32-le variant index, then fields in order.
//   Asset:  0 = Native | 1 = Spl([u8;32])
//   PoolInstruction: 0 = Shield | 1 = Transfer | 2 = Unshield

import { BincodeReader, BincodeWriter } from "./bincode.js";
import { frToBytes } from "./field.js";

export type Asset =
  | { kind: "native" }
  | { kind: "spl"; mint: Uint8Array };

export const Asset = {
  native(): Asset {
    return { kind: "native" };
  },
  spl(mint: Uint8Array): Asset {
    if (mint.length !== 32) throw new Error("mint must be 32 bytes");
    return { kind: "spl", mint };
  },
};

function writeAsset(w: BincodeWriter, asset: Asset): void {
  if (asset.kind === "native") {
    w.u32(0);
  } else {
    w.u32(1).fixedBytes(asset.mint, 32);
  }
}

function readAsset(r: BincodeReader): Asset {
  const tag = r.u32();
  if (tag === 0) return { kind: "native" };
  if (tag === 1) return { kind: "spl", mint: r.fixedBytes(32) };
  throw new Error(`invalid asset tag ${tag}`);
}

export interface ShieldArgs {
  asset: Asset;
  amount: bigint;
  // Opaque secret = Poseidon2(pk, blinding); the node derives the commitment
  // from the public (asset, amount) and this, binding the deposited amount.
  secret: Uint8Array; // 32-byte LE field
  encryptedNote: Uint8Array;
}

export interface TransferArgs {
  proof: Uint8Array; // 256-byte BE Groth16 blob
  root: Uint8Array;
  nullifiers: [Uint8Array, Uint8Array];
  commitments: [Uint8Array, Uint8Array];
  fee: bigint;
  encryptedNotes: [Uint8Array, Uint8Array];
}

export interface UnshieldArgs {
  proof: Uint8Array;
  root: Uint8Array;
  nullifier: Uint8Array;
  changeCommitment: Uint8Array;
  asset: Asset;
  amount: bigint;
  recipient: Uint8Array; // 32-byte pubkey
  encryptedChangeNote: Uint8Array;
}

export function encodeShield(args: ShieldArgs): Uint8Array {
  const w = new BincodeWriter().u32(0);
  writeAsset(w, args.asset);
  w.u64(args.amount);
  w.fixedBytes(args.secret, 32);
  w.vecU8(args.encryptedNote);
  return w.finish();
}

export function encodeTransfer(args: TransferArgs): Uint8Array {
  const w = new BincodeWriter().u32(1);
  w.vecU8(args.proof);
  w.fixedBytes(args.root, 32);
  w.fixedBytes(args.nullifiers[0], 32).fixedBytes(args.nullifiers[1], 32);
  w.fixedBytes(args.commitments[0], 32).fixedBytes(args.commitments[1], 32);
  w.u64(args.fee);
  w.vecU8(args.encryptedNotes[0]).vecU8(args.encryptedNotes[1]);
  return w.finish();
}

export function encodeUnshield(args: UnshieldArgs): Uint8Array {
  const w = new BincodeWriter().u32(2);
  w.vecU8(args.proof);
  w.fixedBytes(args.root, 32);
  w.fixedBytes(args.nullifier, 32);
  w.fixedBytes(args.changeCommitment, 32);
  writeAsset(w, args.asset);
  w.u64(args.amount);
  w.fixedBytes(args.recipient, 32);
  w.vecU8(args.encryptedChangeNote);
  return w.finish();
}

/// Decode helper (mainly for round-trip tests).
export function decodeInstructionTag(bytes: Uint8Array): number {
  return new BincodeReader(bytes).u32();
}

export { readAsset, writeAsset };

// Convenience: build a field-bytes commitment from a bigint.
export function commitmentBytes(value: bigint): Uint8Array {
  return frToBytes(value);
}
