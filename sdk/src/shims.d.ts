// Minimal type shims for untyped deps. Only the surface the SDK uses.

declare module "circomlibjs" {
  export function buildPoseidon(): Promise<
    ((inputs: bigint[]) => Uint8Array) & {
      F: { toObject: (x: Uint8Array) => bigint };
    }
  >;
}

declare module "snarkjs" {
  export const groth16: {
    fullProve(
      input: Record<string, unknown>,
      wasm: string | Uint8Array,
      zkey: string | Uint8Array,
    ): Promise<{ proof: unknown; publicSignals: string[] }>;
    verify(
      vk: unknown,
      publicSignals: string[],
      proof: unknown,
    ): Promise<boolean>;
  };
}
