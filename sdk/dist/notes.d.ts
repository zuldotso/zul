import { FIELD_MODULUS } from "./field.js";
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
export declare function encryptionKeypair(spendingKey: bigint): {
    secret: Uint8Array;
    publicKey: Uint8Array;
};
export declare function encryptNote(note: Note, recipientEncPk: Uint8Array): Uint8Array;
export declare function tryDecryptNote(encrypted: Uint8Array, spendingKey: bigint): Note | null;
export declare class NoteStore {
    private notes;
    add(note: StoredNote): void;
    markSpent(commitment: bigint): void;
    unspent(assetId?: bigint): StoredNote[];
    balance(assetId: bigint): bigint;
    toJSON(): string;
    static fromJSON(json: string): NoteStore;
}
export declare function scanNoteEvent(store: NoteStore, spendingKey: bigint, leafIndex: number, commitment: bigint, encryptedNote: Uint8Array): Promise<StoredNote | null>;
export { FIELD_MODULUS, noteCommitment, nullifier, ownerPk };
