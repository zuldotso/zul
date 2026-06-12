// L2 connection: ZUL speaks the Solana JSON-RPC subset, so web3.js works
// unchanged. This thin wrapper documents the supported surface and centralizes
// the pool program id.

import { Connection, PublicKey, type ConnectionConfig } from "@solana/web3.js";

/// Enshrined shielded-pool program id (matches POOL_PROGRAM_ID in
/// chain/zul-privacy/src/lib.rs).
export const POOL_PROGRAM_ID = new PublicKey(
  new Uint8Array([
    0x05, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
  ]),
);

export const LAMPORTS_PER_ZUL = 1_000_000_000n;

export function connect(
  endpoint: string,
  config?: ConnectionConfig,
): Connection {
  return new Connection(endpoint, config ?? { commitment: "confirmed" });
}
