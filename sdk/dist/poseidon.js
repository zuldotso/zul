// Poseidon hashing via circomlibjs, matching chain/pl2-privacy/src/poseidon.rs
// and the circom circuits. Lazy-initialized (WASM) and cached.
import { buildPoseidon } from "circomlibjs";
import { FIELD_MODULUS } from "./field.js";
let poseidonPromise = null;
async function getPoseidon() {
    if (!poseidonPromise)
        poseidonPromise = buildPoseidon();
    return poseidonPromise;
}
async function hash(inputs) {
    const p = await getPoseidon();
    return p.F.toObject(p(inputs));
}
/// Native PL2 asset id.
export const NATIVE_ASSET_ID = 0n;
/// SPL mint (32-byte pubkey) -> field asset id (LE bytes mod p).
export function assetIdForMint(mint) {
    if (mint.length !== 32)
        throw new Error("mint must be 32 bytes");
    let v = 0n;
    for (let i = 31; i >= 0; i--)
        v = (v << 8n) | BigInt(mint[i]);
    return v % FIELD_MODULUS;
}
export async function ownerPk(spendingKey) {
    return hash([spendingKey]);
}
/// secret = Poseidon2(pk, blinding); revealed at shield time.
export async function noteSecret(ownerPublicKey, blinding) {
    return hash([ownerPublicKey, blinding]);
}
/// commitment from the public (asset, amount) and the opaque secret.
export async function noteCommitmentFromSecret(assetId, amount, secret) {
    return hash([assetId, amount, secret]);
}
export async function noteCommitment(assetId, amount, ownerPublicKey, blinding) {
    return noteCommitmentFromSecret(assetId, amount, await noteSecret(ownerPublicKey, blinding));
}
export async function nullifier(commitment, leafIndex, spendingKey) {
    return hash([commitment, leafIndex, spendingKey]);
}
export async function treeNode(left, right) {
    return hash([left, right]);
}
