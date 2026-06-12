export declare const NATIVE_ASSET_ID = 0n;
export declare function assetIdForMint(mint: Uint8Array): bigint;
export declare function ownerPk(spendingKey: bigint): Promise<bigint>;
export declare function noteSecret(ownerPublicKey: bigint, blinding: bigint): Promise<bigint>;
export declare function noteCommitmentFromSecret(assetId: bigint, amount: bigint, secret: bigint): Promise<bigint>;
export declare function noteCommitment(assetId: bigint, amount: bigint, ownerPublicKey: bigint, blinding: bigint): Promise<bigint>;
export declare function nullifier(commitment: bigint, leafIndex: bigint, spendingKey: bigint): Promise<bigint>;
export declare function treeNode(left: bigint, right: bigint): Promise<bigint>;
