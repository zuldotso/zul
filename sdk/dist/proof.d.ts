export interface Groth16Proof {
    pi_a: [string, string, string];
    pi_b: [[string, string], [string, string], [string, string]];
    pi_c: [string, string, string];
}
export declare function proofToBlob(proof: Groth16Proof): Uint8Array;
export interface ProvingArtifacts {
    wasm: string | Uint8Array;
    zkey: string | Uint8Array;
}
export declare function prove(input: Record<string, unknown>, artifacts: ProvingArtifacts): Promise<{
    proof: Uint8Array;
    publicSignals: string[];
}>;
