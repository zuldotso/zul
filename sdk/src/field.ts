// BN254 scalar field helpers. Wire convention (shared with the Rust node and
// the circuits): field elements are 32-byte little-endian.

export const FIELD_MODULUS =
  21888242871839275222246405745257275088548364400416034343698204186575808495617n;

export type FieldBytes = Uint8Array; // length 32, little-endian

export function frToBytes(value: bigint): FieldBytes {
  let v = ((value % FIELD_MODULUS) + FIELD_MODULUS) % FIELD_MODULUS;
  const out = new Uint8Array(32);
  for (let i = 0; i < 32; i++) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

export function frFromBytes(bytes: FieldBytes): bigint {
  if (bytes.length !== 32) throw new Error("field element must be 32 bytes");
  let v = 0n;
  for (let i = 31; i >= 0; i--) {
    v = (v << 8n) | BigInt(bytes[i]);
  }
  if (v >= FIELD_MODULUS) throw new Error("field element exceeds modulus");
  return v;
}

/// 32-byte big-endian (for Groth16 point coordinates).
export function frToBytesBE(value: bigint): Uint8Array {
  const le = frToBytes(value);
  return le.reverse();
}

export function randomFr(): bigint {
  // 64 bytes of entropy reduced mod p — negligible bias.
  const buf = new Uint8Array(64);
  globalThis.crypto.getRandomValues(buf);
  let v = 0n;
  for (const b of buf) v = (v << 8n) | BigInt(b);
  return v % FIELD_MODULUS;
}
