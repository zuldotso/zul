// Groth16 setup: download/generate powers of tau, then per-circuit phase-2.
// Dev-grade ceremony (single contributor). For anything beyond a testnet,
// replace with a real multi-party ceremony and document the transcript.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { BUILD, PTAU, PTAU_POWER, CIRCUITS } from "./common.mjs";

function snarkjs(args) {
  execFileSync("npx", ["snarkjs", ...args], { stdio: "inherit", shell: process.platform === "win32" });
}

mkdirSync(PTAU, { recursive: true });
const ptauFinal = join(PTAU, `pot${PTAU_POWER}_final.ptau`);

if (!existsSync(ptauFinal)) {
  const pot0 = join(PTAU, `pot${PTAU_POWER}_0.ptau`);
  const pot1 = join(PTAU, `pot${PTAU_POWER}_1.ptau`);
  console.log("powers of tau: new + contribute + prepare");
  snarkjs(["powersoftau", "new", "bn128", String(PTAU_POWER), pot0, "-v"]);
  snarkjs([
    "powersoftau", "contribute", pot0, pot1,
    "--name=zul-dev", "-v", "-e=zul-dev-entropy",
  ]);
  snarkjs(["powersoftau", "prepare", "phase2", pot1, ptauFinal, "-v"]);
}

for (const { name } of CIRCUITS) {
  const dir = join(BUILD, name);
  const r1cs = join(dir, `${name}.r1cs`);
  const zkey0 = join(dir, `${name}_0.zkey`);
  const zkeyFinal = join(dir, `${name}_final.zkey`);
  const vkJson = join(dir, "verification_key.json");

  console.log(`phase-2 setup for ${name} ...`);
  snarkjs(["groth16", "setup", r1cs, ptauFinal, zkey0]);
  snarkjs([
    "zkey", "contribute", zkey0, zkeyFinal,
    `--name=zul-${name}`, "-v", "-e=zul-dev-zkey-entropy",
  ]);
  snarkjs(["zkey", "export", "verificationkey", zkeyFinal, vkJson]);
}
console.log("setup: done");
