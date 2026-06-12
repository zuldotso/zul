import { type Asset } from "./instruction.js";
import { NATIVE_ASSET_ID, ownerPk } from "./poseidon.js";
import { MerkleTree, TREE_DEPTH } from "./merkle.js";
import { type Note, type StoredNote } from "./notes.js";
import { type ProvingArtifacts } from "./proof.js";
export interface ShieldParams {
    asset: Asset;
    assetId: bigint;
    amount: bigint;
    recipientPk: bigint;
    recipientEncPk: Uint8Array;
}
export interface ShieldResult {
    data: Uint8Array;
    note: Note;
    commitment: bigint;
}
export declare function buildShield(params: ShieldParams): Promise<ShieldResult>;
export interface TransferParams {
    spendingKey: bigint;
    assetId: bigint;
    inputs: [StoredNote, StoredNote];
    outputs: [TransferOutput, TransferOutput];
    fee: bigint;
    tree: MerkleTree;
    artifacts: ProvingArtifacts;
}
export interface TransferOutput {
    amount: bigint;
    recipientPk: bigint;
    recipientEncPk: Uint8Array;
}
export interface TransferResult {
    data: Uint8Array;
    outputNotes: Note[];
    nullifiers: [bigint, bigint];
}
export declare function buildTransfer(params: TransferParams): Promise<TransferResult>;
export interface UnshieldParams {
    spendingKey: bigint;
    asset: Asset;
    assetId: bigint;
    input: StoredNote;
    amount: bigint;
    recipient: Uint8Array;
    changePk: bigint;
    changeEncPk: Uint8Array;
    tree: MerkleTree;
    artifacts: ProvingArtifacts;
}
export interface UnshieldResult {
    data: Uint8Array;
    changeNote: Note;
    nullifier: bigint;
}
export declare function buildUnshield(params: UnshieldParams): Promise<UnshieldResult>;
export { TREE_DEPTH, NATIVE_ASSET_ID, ownerPk };
