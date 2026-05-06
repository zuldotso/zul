// Generate a cross-language proof fixture: produce a real Groth16 proof with
// snarkjs and emit it in the in-node verifier's wire format, so a Rust test
// can prove the circuit <-> verifier binding (Poseidon params, public-input
// order, point encoding) is exact.
//
// The witness uses two dummy (amount-0) inputs and two amount-0 outputs:
// value conservation holds trivially and the Merkle membership check is
// disabled, so no real tree/path is needed — yet the proof still exercises
// the full commitment/nullifier hashing and the verifier format.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { groth16 } from "snarkjs";
import { BUILD, ROOT } from "./common.mjs";

function feBE(decimalStr) {
  const buf = Buffer.alloc(32);
  let v = BigInt(decimalStr);
  for (let i = 31; i >= 0; i--) {
    buf[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  if (v !== 0n) throw new Error("field element exceeds 32 bytes");
  return buf;
}
const g1BE = (p) => Buffer.concat([feBE(p[0]), feBE(p[1])]);
const g2BE = (p) =>
  Buffer.concat([feBE(p[0][1]), feBE(p[0][0]), feBE(p[1][1]), feBE(p[1][0])]);

function proofBlob(proof) {
  return Buffer.concat([g1BE(proof.pi_a), g2BE(proof.pi_b), g1BE(proof.pi_c)]);
}

const input = {
  root: "12345678901234567890",
  nullifier: ["0", "0"],         // overwritten by circuit; placeholders
  out_commitment: ["0", "0"],
  fee: "0",
  asset_id: "0",
  in_amount: ["0", "0"],
  in_s: ["111", "222"],
  in_blinding: ["333", "444"],
  in_leaf_index: ["0", "1"],
  in_path: [Array(26).fill("0"), Array(26).fill("0")],
  out_amount: ["0", "0"],
  out_pk: ["555", "666"],
  out_blinding: ["777", "888"],
};

// nullifier/out_commitment are public *inputs* the prover must supply
// consistently. Compute them first with a witness pass that derives them,
// then re-run with the correct values. snarkjs has no "compute outputs" for
// public inputs, so we derive via the circuit's expected formulas using the
// wasm calculator: easiest is to let fullProve fail-fast if inconsistent.
// Instead we precompute with poseidon from circomlibjs.
import { buildPoseidon } from "circomlibjs";

const dir = join(BUILD, "transfer");
const wasm = join(dir, "transfer_js", "transfer.wasm");
const zkey = join(dir, "transfer_final.zkey");

const poseidon = await buildPoseidon();
const F = poseidon.F;
const toDec = (x) => F.toObject(x).toString();

function pk(s) {
  return toDec(poseidon([BigInt(s)]));
}
function secretOf(pkv, blinding) {
  return toDec(poseidon([BigInt(pkv), BigInt(blinding)]));
}
function commitment(asset, amount, pkv, blinding) {
  // Two-level: commitment = Poseidon3(asset, amount, Poseidon2(pk, blinding)).
  const secret = secretOf(pkv, blinding);
  return toDec(poseidon([BigInt(asset), BigInt(amount), BigInt(secret)]));
}
function nullifier(cm, idx, s) {
  return toDec(poseidon([BigInt(cm), BigInt(idx), BigInt(s)]));
}

for (let i = 0; i < 2; i++) {
  const pkv = pk(input.in_s[i]);
  const cm = commitment(input.asset_id, input.in_amount[i], pkv, input.in_blinding[i]);
  input.nullifier[i] = nullifier(cm, input.in_leaf_index[i], input.in_s[i]);
  input.out_commitment[i] = commitment(
    input.asset_id, input.out_amount[i], input.out_pk[i], input.out_blinding[i],
  );
}

console.log("proving transfer fixture ...");
const { proof, publicSignals } = await groth16.fullProve(input, wasm, zkey);

const vkBlob = readFileSync(join(ROOT, "artifacts", "transfer.vk.bin"));
const fixture = {
  circuit: "transfer",
  vk_hex: vkBlob.toString("hex"),
  proof_hex: proofBlob(proof).toString("hex"),
  public_inputs: publicSignals, // decimal strings, in circuit order
};

const outDir = join(ROOT, "..", "chain", "zul-privacy", "tests", "fixtures");
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, "transfer_proof.json"), JSON.stringify(fixture, null, 2));
console.log(`wrote transfer_proof.json (${publicSignals.length} public inputs)`);
