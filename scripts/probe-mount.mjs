import { pathToFileURL } from 'node:url';

const PW = 'C:/Users/Administrator/AppData/Roaming/npm/node_modules/playwright/index.js';
const pw = await import(pathToFileURL(PW).href);
const chromium = pw.chromium || pw.default?.chromium;

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();

const errors = [];
const logs = [];
page.on('console', (m) => logs.push(`[${m.type()}] ${m.text()}`));
page.on('pageerror', (e) => errors.push(`PAGEERROR: ${e.message}`));
page.on('requestfailed', (r) => errors.push(`REQFAIL: ${r.url()} ${r.failure()?.errorText}`));

await page.goto('http://127.0.0.1:1420/', { waitUntil: 'networkidle', timeout: 20000 });

// Wait up to 5s for React to populate #root
await page.waitForFunction(() => document.getElementById('root')?.children.length > 0, { timeout: 5000 })
  .catch(() => {});

const rootChildren = await page.evaluate(() => document.getElementById('root')?.children.length ?? -1);
const bodyText = await page.evaluate(() => document.body.innerText.slice(0, 200));
const hasSpinner = await page.evaluate(() => {
  const el = document.getElementById('root');
  if (!el) return false;
  return el.children.length === 0; // empty => spinner visible via CSS
});

console.log('=== ROOT CHILDREN:', rootChildren);
console.log('=== SPINNER (root empty):', hasSpinner);
console.log('=== BODY:', JSON.stringify(bodyText));
console.log('=== ERRORS:', errors.length ? '\n' + errors.join('\n') : '(none)');
console.log('=== LOGS:', logs.length ? '\n' + logs.join('\n') : '(none)');

await browser.close();
