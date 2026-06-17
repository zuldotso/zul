// Assemble the Zul whitepaper: concatenate content/*.html partials, inline
// figure SVGs, render LaTeX math server-side with KaTeX, wrap in the Paged.js
// shell. Output: whitepaper.html (rendered to PDF by render.mjs).
import { readFileSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import katex from "katex";

const ROOT = dirname(fileURLToPath(import.meta.url));
const CONTENT = join(ROOT, "content");
const FIG = join(ROOT, "figures");

let mathErrors = 0;
const renderMath = (tex, displayMode) => {
  try {
    return katex.renderToString(tex.trim(), {
      displayMode,
      throwOnError: true,
      strict: false,
      macros: {
        "\\F": "\\mathbb{F}",
        "\\Fp": "\\mathbb{F}_p",
        "\\G": "\\mathbb{G}",
        "\\H": "\\mathcal{H}",
        "\\cm": "\\mathsf{cm}",
        "\\nf": "\\mathsf{nf}",
        "\\pk": "\\mathsf{pk}",
        "\\sk": "\\mathsf{sk}",
        "\\Poseidon": "\\mathsf{Poseidon}",
        "\\blake": "\\mathsf{blake3}",
        "\\Adv": "\\mathcal{A}",
        "\\negl": "\\mathsf{negl}",
      },
    });
  } catch (e) {
    mathErrors++;
    console.error(`  ! KaTeX error in ${displayMode ? "display" : "inline"}: ${tex.trim().slice(0, 60)}\n    ${e.message}`);
    return `<span style="color:#c00">[math error]</span>`;
  }
};

const processMath = (html) =>
  html
    .replace(/\$\$([\s\S]+?)\$\$/g, (_, tex) => renderMath(tex, true))
    .replace(/\\\(([\s\S]+?)\\\)/g, (_, tex) => renderMath(tex, false));

const inlineFigures = (html) =>
  html.replace(/@@fig:([\w.-]+)@@/g, (_, name) => {
    const p = join(FIG, name.endsWith(".svg") ? name : name + ".svg");
    if (!existsSync(p)) {
      console.error(`  ! missing figure: ${name}`);
      return `<!-- missing figure ${name} -->`;
    }
    return readFileSync(p, "utf8");
  });

const files = readdirSync(CONTENT).filter((f) => f.endsWith(".html")).sort();
if (files.length === 0) throw new Error("no content partials in content/");
console.log(`Partials: ${files.join(", ")}`);

let body = files.map((f) => `<!-- ${f} -->\n` + readFileSync(join(CONTENT, f), "utf8")).join("\n");
body = inlineFigures(body);
body = processMath(body);

const css = readFileSync(join(ROOT, "styles.css"), "utf8");

const html = `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<title>Zul — A Privacy Layer 2 for the Solana Virtual Machine</title>
<link rel="stylesheet" href="node_modules/katex/dist/katex.min.css" />
<style>
${css}
</style>
<script>window.PagedConfig = { auto: true, after: () => { window.__pagedDone = true; } };</script>
<script src="node_modules/pagedjs/dist/paged.polyfill.js"></script>
</head>
<body>
${body}
</body>
</html>
`;

writeFileSync(join(ROOT, "whitepaper.html"), html);
const words = body.replace(/<[^>]+>/g, " ").split(/\s+/).filter(Boolean).length;
console.log(`Wrote whitepaper.html (${(html.length / 1024).toFixed(0)} KB, ~${words} words, ${mathErrors} math errors)`);
