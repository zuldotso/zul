// Screenshot selected Paged.js pages to PNG for visual verification.
// Usage: node shots.mjs <input.html> <out-prefix> <page#,page#,...>
import puppeteer from "puppeteer-core";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, extname, normalize } from "node:path";

const chrome = [
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
].find((p) => existsSync(p));

const ROOT = process.cwd();
const input = process.argv[2] ?? "whitepaper.html";
const prefix = process.argv[3] ?? "shot";
const wanted = (process.argv[4] ?? "1").split(",").map((n) => parseInt(n, 10));

const TYPES = { ".html":"text/html",".css":"text/css",".js":"application/javascript",".mjs":"application/javascript",".json":"application/json",".map":"application/json",".woff2":"font/woff2",".woff":"font/woff",".ttf":"font/ttf",".svg":"image/svg+xml" };
const server = createServer(async (req, res) => {
  try {
    const fp = normalize(join(ROOT, decodeURIComponent(req.url.split("?")[0])));
    if (!fp.startsWith(ROOT)) return res.writeHead(403).end();
    const d = await readFile(fp);
    res.writeHead(200, { "Content-Type": TYPES[extname(fp)] ?? "application/octet-stream" }).end(d);
  } catch { res.writeHead(404).end(); }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

const browser = await puppeteer.launch({ executablePath: chrome, headless: true, args: ["--no-sandbox", "--force-color-profile=srgb"] });
const page = await browser.newPage();
await page.setViewport({ width: 1240, height: 1754, deviceScaleFactor: 2 });
await page.goto(`http://127.0.0.1:${port}/${input}`, { waitUntil: "networkidle0", timeout: 180000 });
await page.waitForFunction(() => window.__pagedDone === true, { timeout: 300000 });

const total = await page.evaluate(() => document.querySelectorAll(".pagedjs_page").length);
for (const n of wanted) {
  if (n < 1 || n > total) continue;
  const el = await page.evaluateHandle((i) => document.querySelectorAll(".pagedjs_page")[i - 1], n);
  const box = await el.boundingBox();
  if (!box) { console.log("no box for", n); continue; }
  await page.screenshot({ path: `${prefix}-${String(n).padStart(2, "0")}.png`, clip: box });
  console.log("shot", n);
}
await browser.close();
server.close();
console.log(`total pages: ${total}`);
