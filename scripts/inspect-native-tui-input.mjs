import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const [executableArgument, shellArgument, screenshotArgument] = process.argv.slice(2);
assert.ok(
  executableArgument && shellArgument,
  'usage: node scripts/inspect-native-tui-input.mjs <twitty.exe> <probe-shell.exe> [screenshot.png]',
);

const executable = resolve(executableArgument);
const shell = resolve(shellArgument);
const screenshot = screenshotArgument ? resolve(screenshotArgument) : undefined;
const port = 20000 + Math.floor(Math.random() * 1000);
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
const deadline = Date.now() + 15_000;
let browser;
let page;

while (Date.now() < deadline && !page) {
  try {
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    page = browser.contexts()
      .flatMap((context) => context.pages())
      .find((candidate) => !candidate.url().startsWith('devtools://'));
  } catch {
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 25));
  }
}

try {
  assert.ok(page, 'Twitty WebView did not expose a page within 15 seconds');
  await page.waitForSelector('.xterm-rows', { timeout: 10_000 });
  await page.waitForFunction(
    () => Array.from(document.querySelectorAll('.xterm-rows > div'))
      .some((row) => row.textContent?.includes('❯ مرحبا بالعالم')),
    undefined,
    { timeout: 10_000, polling: 20 },
  );

  const result = await page.evaluate(() => {
    const row = Array.from(document.querySelectorAll('.xterm-rows > div'))
      .find((candidate) => candidate.textContent?.includes('❯ مرحبا بالعالم'));
    const bounds = (element) => {
      const rect = element?.getBoundingClientRect();
      return rect ? { left: rect.left, right: rect.right } : null;
    };
    const spans = Array.from(row.querySelectorAll('.xterm-arabic-group > span'))
      .map((span) => ({ text: span.textContent, ...bounds(span) }));
    return {
      text: row.textContent,
      hasCursor: Boolean(row.querySelector('.xterm-cursor')),
      paragraphDirection: getComputedStyle(row).direction,
      wrappers: row.querySelectorAll(':scope > .xterm-arabic-group').length,
      cursor: bounds(row.querySelector('.xterm-cursor')),
      spans,
    };
  });

  const firstWord = result.spans.find((span) => span.text === 'مرحبا');
  const secondWord = result.spans.find((span) => span.text === 'بالعالم');
  assert.equal(result.hasCursor, true, 'native probe must exercise an active cursor row');
  assert.equal(result.paragraphDirection, 'ltr', 'TUI row geometry must stay LTR');
  assert.equal(result.wrappers, 1, 'native TUI Arabic input must use one local RTL group');
  assert.ok(firstWord && secondWord, 'native TUI Arabic words are missing');
  assert.ok(firstWord.left > secondWord.left, 'native Arabic word order is reversed');
  assert.ok(result.cursor, 'native TUI cursor bounds are missing');
  assert.ok(
    result.cursor.right <= secondWord.left + 0.5,
    'native cursor must follow the logical Arabic end on the visual left',
  );

  if (screenshot) await page.screenshot({ path: screenshot });
  console.log(JSON.stringify({ ...result, screenshot: screenshot || null }, null, 2));
} finally {
  await browser?.close();
  child.kill();
}
