// L2 connection: PL2 speaks the Solana JSON-RPC subset, so web3.js works
// unchanged. This thin wrapper documents the supported surface and centralizes
// the pool program id.
import { Connection, PublicKey } from "@solana/web3.js";
/// Enshrined shielded-pool program id (matches POOL_PROGRAM_ID in
/// chain/pl2-privacy/src/lib.rs).
export const POOL_PROGRAM_ID = new PublicKey(new Uint8Array([
    0x05, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
]));
export const LAMPORTS_PER_PL2 = 1000000000n;
export function connect(endpoint, config) {
    return new Connection(endpoint, config ?? { commitment: "confirmed" });
}
