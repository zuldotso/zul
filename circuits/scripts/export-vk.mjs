// Convert snarkjs verification_key.json into the binary blob the in-node
// verifier reads (chain/zul-privacy/src/verifier.rs::parse_verifying_key):
//
//   u32-le nPublic
//   alpha  G1   (x||y, 32B BE each)
//   beta   G2   (x_c1||x_c0||y_c1||y_c0, 32B BE each — imaginary first)
//   gamma  G2
//   delta  G2
//   IC     (nPublic+1) * G1
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { BUILD, KEYS_OUT, CIRCUITS } from "./common.mjs";

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

function g1BE(p) {
  // snarkjs G1 = [x, y, 1]
  return Buffer.concat([feBE(p[0]), feBE(p[1])]);
}

function g2BE(p) {
  // snarkjs G2 = [[x_c0, x_c1], [y_c0, y_c1], [1, 0]]; we emit c1 then c0.
  return Buffer.concat([
    feBE(p[0][1]), feBE(p[0][0]),
    feBE(p[1][1]), feBE(p[1][0]),
  ]);
}

mkdirSync(KEYS_OUT, { recursive: true });

for (const { name, publicInputs } of CIRCUITS) {
  const vk = JSON.parse(
    readFileSync(join(BUILD, name, "verification_key.json"), "utf8"),
  );
  if (vk.nPublic !== publicInputs) {
    throw new Error(
      `${name}: vk.nPublic=${vk.nPublic} expected ${publicInputs}`,
    );
  }
  const header = Buffer.alloc(4);
  header.writeUInt32LE(vk.nPublic, 0);
  const parts = [
    header,
    g1BE(vk.vk_alpha_1),
    g2BE(vk.vk_beta_2),
    g2BE(vk.vk_gamma_2),
    g2BE(vk.vk_delta_2),
    ...vk.IC.map(g1BE),
  ];
  const blob = Buffer.concat(parts);
  const expected = 4 + 64 + 128 * 3 + (vk.nPublic + 1) * 64;
  if (blob.length !== expected) {
    throw new Error(`${name}: blob ${blob.length} != expected ${expected}`);
  }
  writeFileSync(join(KEYS_OUT, `${name}.vk.bin`), blob);
  console.log(`wrote ${name}.vk.bin (${blob.length} bytes)`);
}
console.log("export-keys: done");
