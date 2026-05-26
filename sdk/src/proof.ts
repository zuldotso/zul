// Groth16 proving via snarkjs, with conversion to the node's wire format
// (256-byte big-endian A||B||C blob; matches verifier.rs::parse_proof).

import { groth16 } from "snarkjs";
import { frToBytesBE } from "./field.js";

export interface Groth16Proof {
  pi_a: [string, string, string];
  pi_b: [[string, string], [string, string], [string, string]];
  pi_c: [string, string, string];
}

function g1BE(p: [string, string, string]): Uint8Array {
  const out = new Uint8Array(64);
  out.set(frToBytesBE(BigInt(p[0])), 0);
  out.set(frToBytesBE(BigInt(p[1])), 32);
  return out;
}

function g2BE(p: [[string, string], [string, string], [string, string]]): Uint8Array {
  const out = new Uint8Array(128);
  out.set(frToBytesBE(BigInt(p[0][1])), 0); // x.c1
  out.set(frToBytesBE(BigInt(p[0][0])), 32); // x.c0
  out.set(frToBytesBE(BigInt(p[1][1])), 64); // y.c1
  out.set(frToBytesBE(BigInt(p[1][0])), 96); // y.c0
  return out;
}

/// snarkjs proof JSON -> 256-byte node blob.
export function proofToBlob(proof: Groth16Proof): Uint8Array {
  const out = new Uint8Array(256);
  out.set(g1BE(proof.pi_a), 0);
  out.set(g2BE(proof.pi_b), 64);
  out.set(g1BE(proof.pi_c), 192);
  return out;
}

export interface ProvingArtifacts {
  /// Path or URL to the circuit .wasm.
  wasm: string | Uint8Array;
  /// Path or URL to the final .zkey.
  zkey: string | Uint8Array;
}

/// Prove and return both the node-format blob and the raw public signals.
export async function prove(
  input: Record<string, unknown>,
  artifacts: ProvingArtifacts,
): Promise<{ proof: Uint8Array; publicSignals: string[] }> {
  const { proof, publicSignals } = await groth16.fullProve(
    input,
    artifacts.wasm,
    artifacts.zkey,
  );
  return { proof: proofToBlob(proof as Groth16Proof), publicSignals };
}
