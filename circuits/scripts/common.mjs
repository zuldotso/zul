import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
export const ROOT = join(here, "..");
export const BUILD = join(ROOT, "build");
export const PTAU = join(ROOT, "ptau");
// Verifying keys are PUBLIC parameters (not secrets) — the node loads them
// at startup. Kept out of the gitignored keys/ tree on purpose.
export const KEYS_OUT = join(ROOT, "artifacts");

/// The circuits to build. `name` matches the .circom file stem.
export const CIRCUITS = [
  { name: "transfer", publicInputs: 6 },
  { name: "unshield", publicInputs: 6 },
];

/// Powers of tau: 2^16 constraints is plenty for a depth-26 2x2 join-split.
export const PTAU_POWER = 16;
