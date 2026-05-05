// Compile each circuit to r1cs + wasm. Requires `circom` on PATH (2.1.x).
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { ROOT, BUILD, CIRCUITS } from "./common.mjs";

mkdirSync(BUILD, { recursive: true });

for (const { name } of CIRCUITS) {
  const out = join(BUILD, name);
  mkdirSync(out, { recursive: true });
  console.log(`compiling ${name}.circom ...`);
  execFileSync(
    "circom",
    [
      join(ROOT, `${name}.circom`),
      "--r1cs",
      "--wasm",
      "--sym",
      "-o",
      out,
      "-l",
      ROOT,
    ],
    { stdio: "inherit" },
  );
}
console.log("compile: done");
