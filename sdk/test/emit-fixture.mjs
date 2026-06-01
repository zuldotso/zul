// Emit a hex fixture of SDK-encoded PoolInstructions so a Rust test can
// decode them and confirm byte-for-byte compatibility with serde/bincode.
import { writeFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { Asset, encodeShield, encodeTransfer, encodeUnshield } from "../dist/instruction.js";

const here = dirname(fileURLToPath(import.meta.url));
const hex = (u8) => Buffer.from(u8).toString("hex");

const shield = encodeShield({
  asset: Asset.native(),
  amount: 1234567890n,
  secret: new Uint8Array(32).fill(3),
  encryptedNote: new Uint8Array([0xaa, 0xbb, 0xcc]),
});

const splMint = new Uint8Array(32).fill(9);
const transfer = encodeTransfer({
  proof: new Uint8Array(256).fill(2),
  root: new Uint8Array(32).fill(1),
  nullifiers: [new Uint8Array(32).fill(4), new Uint8Array(32).fill(5)],
  commitments: [new Uint8Array(32).fill(6), new Uint8Array(32).fill(7)],
  fee: 5000n,
  encryptedNotes: [new Uint8Array([1, 2]), new Uint8Array([3, 4, 5])],
});

const unshield = encodeUnshield({
  proof: new Uint8Array(256).fill(8),
  root: new Uint8Array(32).fill(1),
  nullifier: new Uint8Array(32).fill(4),
  changeCommitment: new Uint8Array(32).fill(5),
  asset: Asset.spl(splMint),
  amount: 900n,
  recipient: new Uint8Array(32).fill(8),
  encryptedChangeNote: new Uint8Array([7]),
});

const fixture = {
  shield: hex(shield),
  transfer: hex(transfer),
  unshield: hex(unshield),
  spl_mint: hex(splMint),
};

const outDir = join(here, "..", "..", "chain", "zul-privacy", "tests", "fixtures");
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, "sdk_instructions.json"), JSON.stringify(fixture, null, 2));
console.log("wrote sdk_instructions.json");
