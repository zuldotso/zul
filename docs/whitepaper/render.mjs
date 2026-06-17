// Render a Paged.js HTML document to PDF via the system Chrome.
// Serves the folder over local HTTP (so Paged.js can XHR stylesheets and load
// fonts without file:// CORS failures), then prints with puppeteer-core.
// Usage: node render.mjs <input.html> <output.pdf>
import puppeteer from "puppeteer-core";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { resolve, join, extname, normalize } from "node:path";

const CHROME_CANDIDATES = [
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
];
const chrome = CHROME_CANDIDATES.find((p) => existsSync(p));
if (!chrome) throw new Error("No Chrome/Edge binary found");

const ROOT = process.cwd();
const input = process.argv[2] ?? "whitepaper.html";
const output = resolve(process.argv[3] ?? "Zul-Whitepaper.pdf");

const TYPES = {
  ".html": "text/html", ".css": "text/css", ".js": "application/javascript",
  ".mjs": "application/javascript", ".json": "application/json", ".map": "application/json",
  ".woff2": "font/woff2", ".woff": "font/woff", ".ttf": "font/ttf",
  ".svg": "image/svg+xml", ".png": "image/png",
};
const server = createServer(async (req, res) => {
  try {
    const urlPath = decodeURIComponent(req.url.split("?")[0]);
    const filePath = normalize(join(ROOT, urlPath));
    if (!filePath.startsWith(ROOT)) { res.writeHead(403).end(); return; }
    const data = await readFile(filePath);
    res.writeHead(200, { "Content-Type": TYPES[extname(filePath)] ?? "application/octet-stream" });
    res.end(data);
  } catch {
    res.writeHead(404).end("not found");
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

const browser = await puppeteer.launch({
  executablePath: chrome,
  headless: true,
  args: ["--no-sandbox", "--font-render-hinting=none", "--force-color-profile=srgb"],
});
const page = await browser.newPage();
page.on("pageerror", (e) => console.log("  [pageerror]", e.message.slice(0, 200)));
page.on("requestfailed", (r) => console.log("  [reqfail]", r.url().slice(-60), r.failure() && r.failure().errorText));

await page.goto(`http://127.0.0.1:${port}/${input}`, { waitUntil: "networkidle0", timeout: 180000 });
await page.waitForFunction(() => window.__pagedDone === true, { timeout: 300000 });

const pages = await page.evaluate(() => document.querySelectorAll(".pagedjs_page").length);
await page.pdf({ path: output, printBackground: true, preferCSSPageSize: true, displayHeaderFooter: false });
await browser.close();
server.close();
console.log(`Rendered ${pages} pages -> ${output}`);
