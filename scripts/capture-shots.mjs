import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const pw = await import(pathToFileURL('C:/Users/Administrator/AppData/Roaming/npm/node_modules/playwright/index.js').href);
const chromium = pw.chromium || pw.default?.chromium;

const dist = fileURLToPath(new URL('../dist/', import.meta.url));
const outDir = fileURLToPath(new URL('../docs/assets/', import.meta.url));
await mkdir(outDir, { recursive: true });

const types = {
  '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css',
  '.woff2': 'font/woff2', '.png': 'image/png', '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon', '.json': 'application/json', '.map': 'application/json',
};
const server = createServer(async (req, res) => {
  try {
    let p = decodeURIComponent(req.url.split('?')[0]);
    if (p === '/') p = '/index.html';
    const buf = await readFile(normalize(join(dist, p)));
    res.writeHead(200, { 'content-type': types[extname(p)] || 'application/octet-stream' });
    res.end(buf);
  } catch { res.writeHead(404); res.end('404'); }
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const port = server.address().port;

const browser = await chromium.launch();

async function shot(name, width, height, clip) {
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 2 });
  // Headless WebGL canvases don't read back into page screenshots. For docs
  // capture only, deny the WebGL context so the app's try/catch falls back to
  // the DOM renderer (real text spans) — the character joiner + browser Arabic
  // shaping still apply, so RTL renders identically to the GPU path.
  await page.addInitScript(() => {
    const orig = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (type, attrs) {
      if (type === 'webgl2' || type === 'webgl') return null;
      return orig.call(this, type, attrs);
    };
  });
  await page.goto(`http://127.0.0.1:${port}/`, { waitUntil: 'networkidle' });
  await page.waitForSelector('.xterm-screen', { timeout: 5000 });
  await page.waitForTimeout(1200);
  await page.screenshot({ path: join(outDir, name), ...(clip ? { clip } : {}) });
  await page.close();
  console.log('saved', name, `${width}x${height}`);
}

await shot('screenshot-main.png', 1180, 760);
await shot('screenshot-wide.png', 1320, 600);
// Tight hero crop: the populated top region + the status bar look, no dead space.
await shot('screenshot-hero.png', 1180, 470, { x: 0, y: 0, width: 1180, height: 470 });

await browser.close();
server.close();
