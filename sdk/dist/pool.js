// High-level shielded pool client: builds the PoolInstruction payloads and
// generates the proofs. The returned `data` goes into a TransactionInstruction
// targeting POOL_PROGRAM_ID; the node's privacy processor consumes it.
//
// Account metas are assembled by the caller/wallet per operation (pool state,
// vault, user accounts); this module owns the cryptographic construction.
import { encodeShield, encodeTransfer, encodeUnshield, } from "./instruction.js";
import { frToBytes, randomFr } from "./field.js";
import { NATIVE_ASSET_ID, noteCommitment, noteSecret, nullifier, ownerPk, } from "./poseidon.js";
import { TREE_DEPTH } from "./merkle.js";
import { encryptNote } from "./notes.js";
import { prove } from "./proof.js";
/// Build a shield instruction: create a fresh note for the recipient. The
/// instruction carries only the opaque `secret`; the node derives the
/// commitment from the public (asset, amount), binding the deposited amount.
export async function buildShield(params) {
    const blinding = randomFr();
    const note = {
        assetId: params.assetId,
        amount: params.amount,
        ownerPk: params.recipientPk,
        blinding,
    };
    const secret = await noteSecret(note.ownerPk, note.blinding);
    const commitment = await noteCommitment(note.assetId, note.amount, note.ownerPk, note.blinding);
    const encryptedNote = encryptNote(note, params.recipientEncPk);
    const data = encodeShield({
        asset: params.asset,
        amount: params.amount,
        secret: frToBytes(secret),
        encryptedNote,
    });
    return { data, note, commitment };
}
export async function buildTransfer(params) {
    const root = await params.tree.root();
    const inNullifiers = [];
    const inPaths = [];
    for (const note of params.inputs) {
        const cm = await noteCommitment(note.assetId, note.amount, note.ownerPk, note.blinding);
        inNullifiers.push(await nullifier(cm, BigInt(note.leafIndex), params.spendingKey));
        inPaths.push(await params.tree.path(note.leafIndex));
    }
    const outputNotes = [];
    const outBlindings = [];
    const outCommitments = [];
    const encryptedNotes = [];
    for (const out of params.outputs) {
        const blinding = randomFr();
        const note = {
            assetId: params.assetId,
            amount: out.amount,
            ownerPk: out.recipientPk,
            blinding,
        };
        outputNotes.push(note);
        outBlindings.push(blinding);
        outCommitments.push(await noteCommitment(note.assetId, note.amount, note.ownerPk, note.blinding));
        encryptedNotes.push(encryptNote(note, out.recipientEncPk));
    }
    const input = {
        root,
        nullifier: inNullifiers,
        out_commitment: outCommitments,
        fee: params.fee,
        asset_id: params.assetId,
        in_amount: params.inputs.map((n) => n.amount),
        in_s: params.inputs.map(() => params.spendingKey),
        in_blinding: params.inputs.map((n) => n.blinding),
        in_leaf_index: params.inputs.map((n) => BigInt(n.leafIndex)),
        in_path: inPaths,
        out_amount: params.outputs.map((o) => o.amount),
        out_pk: params.outputs.map((o) => o.recipientPk),
        out_blinding: outBlindings,
    };
    const { proof } = await prove(input, params.artifacts);
    const data = encodeTransfer({
        proof,
        root: frToBytes(root),
        nullifiers: [frToBytes(inNullifiers[0]), frToBytes(inNullifiers[1])],
        commitments: [frToBytes(outCommitments[0]), frToBytes(outCommitments[1])],
        fee: params.fee,
        encryptedNotes: [encryptedNotes[0], encryptedNotes[1]],
    });
    return {
        data,
        outputNotes,
        nullifiers: [inNullifiers[0], inNullifiers[1]],
    };
}
export async function buildUnshield(params) {
    const root = await params.tree.root();
    const note = params.input;
    const inCm = await noteCommitment(note.assetId, note.amount, note.ownerPk, note.blinding);
    const nf = await nullifier(inCm, BigInt(note.leafIndex), params.spendingKey);
    const path = await params.tree.path(note.leafIndex);
    const changeAmount = note.amount - params.amount;
    if (changeAmount < 0n)
        throw new Error("unshield amount exceeds note value");
    const changeBlinding = randomFr();
    const changeNote = {
        assetId: params.assetId,
        amount: changeAmount,
        ownerPk: params.changePk,
        blinding: changeBlinding,
    };
    const changeCommitment = await noteCommitment(changeNote.assetId, changeNote.amount, changeNote.ownerPk, changeNote.blinding);
    // Recipient as a field element (LE mod p), matching processor.rs.
    let recipientFr = 0n;
    for (let i = 31; i >= 0; i--)
        recipientFr = (recipientFr << 8n) | BigInt(params.recipient[i]);
    const input = {
        root,
        nullifier: nf,
        change_commitment: changeCommitment,
        asset_id: params.assetId,
        amount: params.amount,
        recipient: recipientFr,
        in_amount: note.amount,
        in_s: params.spendingKey,
        in_blinding: note.blinding,
        in_leaf_index: BigInt(note.leafIndex),
        in_path: path,
        change_amount: changeAmount,
        change_pk: params.changePk,
        change_blinding: changeBlinding,
    };
    const { proof } = await prove(input, params.artifacts);
    const data = encodeUnshield({
        proof,
        root: frToBytes(root),
        nullifier: frToBytes(nf),
        changeCommitment: frToBytes(changeCommitment),
        asset: params.asset,
        amount: params.amount,
        recipient: params.recipient,
        encryptedChangeNote: encryptNote(changeNote, params.changeEncPk),
    });
    return { data, changeNote, nullifier: nf };
}
export { TREE_DEPTH, NATIVE_ASSET_ID, ownerPk };
