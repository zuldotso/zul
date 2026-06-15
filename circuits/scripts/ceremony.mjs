// Real Groth16 phase-2 ceremony for a value-bearing (mainnet) launch.
//
// Unlike scripts/setup.mjs (fixed entropy, single contributor → forgeable),
// this:
//   1. uses a TRUSTED phase-1 (the Hermez perpetual powers-of-tau), so we don't
//      self-generate phase-1 (which would itself be a single-party setup),
//   2. takes MANY independent phase-2 contributions, each on a different machine
//      with real entropy the operator never reveals,
//   3. closes with a PUBLIC random beacon, then verifies the transcript.
//
// The security assumption is "at least one honest contributor", so run it with
// several unrelated participants. This script orchestrates one step at a time;
// the coordinator passes each intermediate .zkey to the next participant.
//
// Usage (see docs/MAINNET-CEREMONY.md for the full runbook):
//   node scripts/ceremony.mjs init                       # download ptau + zkey_0
//   node scripts/ceremony.mjs contribute <circuit> <in.zkey> <out.zkey> "<name>"
//   node scripts/ceremony.mjs finalize  <circuit> <last.zkey>   # beacon + verify + vk
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { BUILD, PTAU, PTAU_POWER, CIRCUITS } from "./common.mjs";

function snarkjs(args) {
  execFileSync("npx", ["snarkjs", ...args], {
    stdio: "inherit",
    shell: process.platform === "win32",
  });
}

// Hermez perpetual powers of tau (community phase-1, widely trusted). The power
// must be ≥ the circuit's constraint count; PTAU_POWER is what the circuits were
// compiled against.
const PTAU_NAME = `powersOfTau28_hez_final_${PTAU_POWER}.ptau`;
const PTAU_URL = `https://storage.googleapis.com/zkevm/ptau/${PTAU_NAME}`;
const ptauFinal = join(PTAU, PTAU_NAME);

function ensurePtau() {
  mkdirSync(PTAU, { recursive: true });
  if (existsSync(ptauFinal)) return;
  console.log(`downloading trusted phase-1: ${PTAU_URL}`);
  // curl is available on the ceremony machines; verify the hash out-of-band
  // against the published Hermez ceremony attestation (see the runbook).
  execFileSync("curl", ["-L", "-o", ptauFinal, PTAU_URL], { stdio: "inherit" });
}

const [, , cmd, ...rest] = process.argv;

if (cmd === "init") {
  ensurePtau();
  for (const { name } of CIRCUITS) {
    const r1cs = join(BUILD, name, `${name}.r1cs`);
    const zkey0 = join(BUILD, name, `${name}_0000.zkey`);
    console.log(`phase-2 init (${name}): groth16 setup → ${zkey0}`);
    snarkjs(["groth16", "setup", r1cs, ptauFinal, zkey0]);
    console.log(`  hand ${zkey0} to the first contributor.`);
  }
} else if (cmd === "contribute") {
  const [circuit, inZkey, outZkey, name] = rest;
  if (!circuit || !inZkey || !outZkey || !name) {
    throw new Error('usage: contribute <circuit> <in.zkey> <out.zkey> "<name>"');
  }
  // No -e: snarkjs prompts for entropy interactively so it is never captured in
  // shell history or argv. Each contributor MUST use fresh, secret randomness.
  console.log(`contribution by "${name}" on ${circuit}: ${inZkey} → ${outZkey}`);
  snarkjs(["zkey", "contribute", inZkey, outZkey, `--name=${name}`, "-v"]);
  console.log(`  verify, then pass ${outZkey} to the next contributor.`);
} else if (cmd === "finalize") {
  const [circuit, lastZkey] = rest;
  if (!circuit || !lastZkey) throw new Error("usage: finalize <circuit> <last.zkey>");
  ensurePtau();
  const r1cs = join(BUILD, circuit, `${circuit}.r1cs`);
  const finalZkey = join(BUILD, circuit, `${circuit}_final.zkey`);
  const vkJson = join(BUILD, circuit, "verification_key.json");
  // Public beacon: a verifiable, unpredictable value (e.g. a future Bitcoin
  // block hash) applied as the final contribution so no contributor controls
  // the last step. Record the beacon source in the transcript.
  const beacon = process.env.ZUL_BEACON;
  if (!beacon) throw new Error("set ZUL_BEACON=<hex> (a public random beacon, e.g. a BTC block hash)");
  console.log(`finalize (${circuit}): beacon → verify → export vk`);
  snarkjs(["zkey", "beacon", lastZkey, finalZkey, beacon, "10", "-n=final-beacon"]);
  snarkjs(["zkey", "verify", r1cs, ptauFinal, finalZkey]);
  snarkjs(["zkey", "export", "verificationkey", finalZkey, vkJson]);
  console.log(`  ${finalZkey} + ${vkJson} ready. Run export-keys to emit *.vk.bin.`);
} else {
  console.log("commands: init | contribute <circuit> <in> <out> <name> | finalize <circuit> <last>");
  process.exit(1);
}
