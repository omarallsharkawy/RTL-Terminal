import { createServer } from 'node:http';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const playwright = await import(
  pathToFileURL(
    'C:/Users/Administrator/AppData/Roaming/npm/node_modules/playwright/index.js',
  ).href
);
const chromium = playwright.chromium || playwright.default?.chromium;
const dist = fileURLToPath(new URL('../dist/', import.meta.url));
const types = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.woff2': 'font/woff2',
  '.png': 'image/png',
};

const server = createServer(async (request, response) => {
  try {
    let path = decodeURIComponent(request.url.split('?')[0]);
    if (path === '/') path = '/index.html';
    const body = await readFile(normalize(join(dist, path)));
    response.writeHead(200, {
      'content-type': types[extname(path)] || 'application/octet-stream',
    });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end('404');
  }
});

await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const port = server.address().port;
const browser = await chromium.launch();
const page = await browser.newPage({
  viewport: { width: 1536, height: 830 },
  deviceScaleFactor: 1.25,
});

await page.goto(`http://127.0.0.1:${port}/`, { waitUntil: 'networkidle' });
await page.waitForSelector('.xterm-rows');
await page.waitForTimeout(6500);

const result = await page.evaluate(() => {
  const arabic = /[\u0600-\u06ff\u0750-\u077f\u08a0-\u08ff]/;
  const rows = Array.from(document.querySelectorAll('.xterm-rows > div'));
  const row = rows.find((candidate) => candidate.textContent?.includes('English stays LTR'));
  if (!row) throw new Error('Mixed Arabic demo row was not rendered');

  const rowStyle = getComputedStyle(row);
  const canvas = document.createElement('canvas');
  const context = canvas.getContext('2d');
  context.font = `${rowStyle.fontWeight} ${rowStyle.fontSize} ${rowStyle.fontFamily}`;

  return {
    overlayDisplay: getComputedStyle(document.getElementById('err-overlay')).display,
    rootChildren: document.getElementById('root')?.children.length || 0,
    rowText: row.textContent,
    rowWidth: row.getBoundingClientRect().width,
    rowFont: context.font,
    spans: Array.from(row.querySelectorAll('span'))
      .filter((span) => arabic.test(span.textContent || ''))
      .map((span) => {
        const style = getComputedStyle(span);
        const text = span.textContent || '';
        return {
          text,
          className: span.className,
          inlineStyle: span.getAttribute('style'),
          width: span.getBoundingClientRect().width,
          naturalWidth: context.measureText(text).width,
          letterSpacing: style.letterSpacing,
          wordSpacing: style.wordSpacing,
          direction: style.direction,
          unicodeBidi: style.unicodeBidi,
        };
      }),
  };
});

console.log(JSON.stringify(result, null, 2));
assert.equal(result.overlayDisplay, 'none', 'runtime error overlay must stay hidden after mount');
assert.ok(result.rootChildren > 0, 'React root must remain mounted');
assert.ok(result.spans.length > 0, 'Arabic renderer span must exist');
for (const span of result.spans) {
  assert.match(span.className, /xterm-arabic-run/, 'Arabic span must receive layout class');
  assert.equal(span.direction, 'rtl', 'Arabic span must receive RTL direction');
  assert.equal(
    span.unicodeBidi,
    'normal',
    'adjacent ANSI-styled Arabic spans must participate in one BiDi sequence',
  );
  assert.ok(parseFloat(span.wordSpacing) < 0, 'Arabic word spacing must be compacted');
}
await browser.close();
server.close();
