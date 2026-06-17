# Zul Whitepaper

Source for **`Zul-Whitepaper.pdf`** — *A Privacy Layer 2 for the Solana Virtual Machine* (v1.0, ~47 pp).

## Build

```sh
npm install                              # katex, pagedjs, puppeteer-core
node build.mjs                           # content/ + figures/ + math  -> whitepaper.html
node render.mjs whitepaper.html Zul-Whitepaper.pdf   # paginate + print via system Chrome
```

`build.mjs` concatenates the `content/*.html` partials (sorted by filename), inlines
each `@@fig:name@@` from `figures/*.svg`, and renders inline `\(…\)` / display `$$…$$`
math server-side with KaTeX. `render.mjs` serves the folder over localhost, lets
Paged.js paginate (running heads, page numbers, dotted-leader TOC via `target-counter`),
and prints to PDF with `puppeteer-core` driving the installed Chrome.

`shots.mjs <html> <prefix> <pages>` renders selected pages to PNG for visual review.

## Layout

```
content/   ordered HTML partials (cover, abstract, TOC, §1–§14, refs, appendices)
figures/   hand-authored SVG diagrams (architecture, SMT, settlement, bridge,
           note, lifecycle, join-split, A→B ladder)
styles.css print stylesheet (@page rules, theorem/figure/table styling)
```

Math: write `\(inline\)` and `$$display$$`; shorthand macros (`\cm`, `\nf`, `\H`,
`\Fp`, `\negl`, …) are defined in `build.mjs`. Figures are referenced by `@@fig:name@@`.
