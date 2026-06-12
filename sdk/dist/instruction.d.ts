import { BincodeReader, BincodeWriter } from "./bincode.js";
export type Asset = {
    kind: "native";
} | {
    kind: "spl";
    mint: Uint8Array;
};
export declare const Asset: {
    native(): Asset;
    spl(mint: Uint8Array): Asset;
};
declare function writeAsset(w: BincodeWriter, asset: Asset): void;
declare function readAsset(r: BincodeReader): Asset;
export interface ShieldArgs {
    asset: Asset;
    amount: bigint;
    secret: Uint8Array;
    encryptedNote: Uint8Array;
}
export interface TransferArgs {
    proof: Uint8Array;
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
    recipient: Uint8Array;
    encryptedChangeNote: Uint8Array;
}
export declare function encodeShield(args: ShieldArgs): Uint8Array;
export declare function encodeTransfer(args: TransferArgs): Uint8Array;
export declare function encodeUnshield(args: UnshieldArgs): Uint8Array;
export declare function decodeInstructionTag(bytes: Uint8Array): number;
export { readAsset, writeAsset };
export declare function commitmentBytes(value: bigint): Uint8Array;
