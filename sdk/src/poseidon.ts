// Poseidon hashing via circomlibjs, matching chain/zul-privacy/src/poseidon.rs
// and the circom circuits. Lazy-initialized (WASM) and cached.

import { buildPoseidon } from "circomlibjs";
import { FIELD_MODULUS } from "./field.js";

type PoseidonFn = ((inputs: bigint[]) => Uint8Array) & {
  F: { toObject: (x: Uint8Array) => bigint };
};

let poseidonPromise: Promise<PoseidonFn> | null = null;

async function getPoseidon(): Promise<PoseidonFn> {
  if (!poseidonPromise) poseidonPromise = buildPoseidon() as Promise<PoseidonFn>;
  return poseidonPromise;
}

async function hash(inputs: bigint[]): Promise<bigint> {
  const p = await getPoseidon();
  return p.F.toObject(p(inputs));
}

/// Native ZUL asset id.
export const NATIVE_ASSET_ID = 0n;

/// SPL mint (32-byte pubkey) -> field asset id (LE bytes mod p).
export function assetIdForMint(mint: Uint8Array): bigint {
  if (mint.length !== 32) throw new Error("mint must be 32 bytes");
  let v = 0n;
  for (let i = 31; i >= 0; i--) v = (v << 8n) | BigInt(mint[i]);
  return v % FIELD_MODULUS;
}

export async function ownerPk(spendingKey: bigint): Promise<bigint> {
  return hash([spendingKey]);
}

/// secret = Poseidon2(pk, blinding); revealed at shield time.
export async function noteSecret(
  ownerPublicKey: bigint,
  blinding: bigint,
): Promise<bigint> {
  return hash([ownerPublicKey, blinding]);
}

/// commitment from the public (asset, amount) and the opaque secret.
export async function noteCommitmentFromSecret(
  assetId: bigint,
  amount: bigint,
  secret: bigint,
): Promise<bigint> {
  return hash([assetId, amount, secret]);
}

export async function noteCommitment(
  assetId: bigint,
  amount: bigint,
  ownerPublicKey: bigint,
  blinding: bigint,
): Promise<bigint> {
  return noteCommitmentFromSecret(
    assetId,
    amount,
    await noteSecret(ownerPublicKey, blinding),
  );
}

export async function nullifier(
  commitment: bigint,
  leafIndex: bigint,
  spendingKey: bigint,
): Promise<bigint> {
  return hash([commitment, leafIndex, spendingKey]);
}

export async function treeNode(left: bigint, right: bigint): Promise<bigint> {
  return hash([left, right]);
}
