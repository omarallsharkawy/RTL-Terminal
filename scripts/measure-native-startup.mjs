import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { performance } from 'node:perf_hooks';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [executableArgument, shellArgument, screenshotArgument] = process.argv.slice(2);
assert.ok(executableArgument, 'usage: node scripts/measure-native-startup.mjs <twitty.exe> <probe-shell.exe> [screenshot.png]');
assert.ok(shellArgument, 'a probe shell that prints ARABIC_LAYOUT_PROBE is required');

const executable = resolve(executableArgument);
const shell = resolve(shellArgument);
const screenshot = screenshotArgument ? resolve(screenshotArgument) : undefined;
const port = 19000 + Math.floor(Math.random() * 1000);
const marker = 'ARABIC_LAYOUT_PROBE';
const startedAt = performance.now();
const child = spawn(executable, [], {
  env: {
    ...process.env,
    TWITTY_SHELL: shell,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
  },
  detached: false,
  stdio: 'ignore',
  windowsHide: true,
});

const playwright = await import(
  pathToFileURL(
    'C:/Users/Administrator/AppData/Roaming/npm/node_modules/playwright/index.js',
  ).href,
);
const chromium = playwright.chromium || playwright.default?.chromium;
const deadline = performance.now() + 15_000;
let browser;
let page;

while (performance.now() < deadline && !page) {
  try {
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    const pages = browser.contexts().flatMap((context) => context.pages());
    page = pages.find((candidate) => !candidate.url().startsWith('devtools://'));
  } catch {
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 20));
  }
}

assert.ok(page, 'Twitty WebView did not expose a page within 15 seconds');
await page.waitForSelector('.xterm-rows', { timeout: 10_000 });
await page.waitForFunction(
  (expected) => document.querySelector('.xterm-rows')?.textContent?.includes(expected),
  marker,
  { timeout: 10_000, polling: 10 },
);
const firstPtyOutputMs = performance.now() - startedAt;
await page.waitForFunction(
  () => document.querySelector('.xterm-rows')?.textContent?.includes('STYLED='),
  undefined,
  { timeout: 5_000, polling: 10 },
);
const arabicRows = await page.evaluate(() => (
  Array.from(document.querySelectorAll('.xterm-rows > div'))
    .filter((row) => /^(PLAIN|STYLED)=/.test(row.textContent || ''))
    .map((row) => ({
      text: row.textContent,
      spans: Array.from(row.querySelectorAll('span')).map((span) => {
        const style = getComputedStyle(span);
        const rect = span.getBoundingClientRect();
        return {
          text: span.textContent,
          className: span.className,
          display: style.display,
          direction: style.direction,
          unicodeBidi: style.unicodeBidi,
          color: style.color,
          backgroundColor: style.backgroundColor,
          left: Number(rect.left.toFixed(1)),
          right: Number(rect.right.toFixed(1)),
        };
      }),
    }))
));
const rendererStyles = await page.evaluate(() => (
  Array.from(document.querySelectorAll('.xterm-screen style')).map((style) => ({
    sheetLoaded: Boolean(style.sheet),
    rules: style.sheet?.cssRules?.length || 0,
  }))
));
assert.ok(
  rendererStyles.length >= 3 && rendererStyles.every((style) => style.sheetLoaded),
  `xterm dynamic styles must pass the native CSP: ${JSON.stringify(rendererStyles)}`,
);
assert.ok(
  rendererStyles.some((style) => style.rules > 700),
  `xterm ANSI palette stylesheet is missing: ${JSON.stringify(rendererStyles)}`,
);
const styledArabic = arabicRows
  .find((row) => row.text?.startsWith('STYLED='))
  ?.spans.filter((span) => span.className.includes('xterm-arabic-run'));
assert.ok(
  styledArabic?.length >= 2,
  `styled Arabic probe must produce multiple renderer spans: ${JSON.stringify(arabicRows)}`,
);
assert.ok(
  styledArabic[0].left > styledArabic[1].left,
  'the first logical Arabic word must render to the visual right across ANSI spans',
);
assert.notEqual(
  styledArabic[0].color,
  styledArabic[1].color,
  'ANSI-styled Arabic words must retain distinct computed colors',
);

if (screenshot) {
  await page.screenshot({ path: screenshot });
}

console.log(JSON.stringify({
  processId: child.pid,
  firstPtyOutputMs: Number(firstPtyOutputMs.toFixed(1)),
  screenshot: screenshot || null,
  rendererStyles,
  arabicRows,
}));
await browser.close();
child.unref();
process.exit(0);
