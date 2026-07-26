import assert from 'node:assert/strict';
import { execFile as execFileCallback, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const execFile = promisify(execFileCallback);
const [executableArgument, cyclesArgument = '8'] = process.argv.slice(2);
assert.ok(
  executableArgument,
  'usage: node scripts/measure-pty-lifecycle.mjs <twitty.exe> [recovery-cycles]',
);

const executable = resolve(executableArgument);
const cycles = Math.max(2, Number.parseInt(cyclesArgument, 10) || 8);
const port = 22000 + Math.floor(Math.random() * 500);
const child = spawn(executable, [], {
  env: {
    ...process.env,
    TWITTY_SHELL: `${process.env.WINDIR || 'C:\\Windows'}\\System32\\where.exe`,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
  },
  stdio: 'ignore',
  windowsHide: true,
});

const playwright = await import(
  pathToFileURL(
    'C:/Users/Administrator/AppData/Roaming/npm/node_modules/playwright/index.js',
  ).href,
);
const chromium = playwright.chromium || playwright.default?.chromium;
let browser;
let page;
const deadline = Date.now() + 15_000;

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

async function processMetrics() {
  const command = [
    `$p = Get-Process -Id ${child.pid} -ErrorAction Stop;`,
    '[pscustomobject]@{',
    'Handles=$p.HandleCount;',
    'Threads=$p.Threads.Count;',
    'PrivateBytes=$p.PrivateMemorySize64',
    '} | ConvertTo-Json -Compress',
  ].join('');
  const { stdout } = await execFile(
    'powershell.exe',
    ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', command],
    { windowsHide: true },
  );
  return JSON.parse(stdout.trim());
}

const samples = [];
try {
  assert.ok(page, 'Twitty WebView did not expose a page within 15 seconds');
  const recovery = page.locator('.recovery-notice');
  await recovery.waitFor({ state: 'visible', timeout: 15_000 });

  for (let cycle = 0; cycle < cycles; cycle += 1) {
    await page.waitForTimeout(150);
    samples.push({ cycle, ...(await processMetrics()) });
    if (cycle === cycles - 1) break;

    await recovery.locator('button').click();
    await recovery.waitFor({ state: 'hidden', timeout: 5_000 });
    await recovery.waitFor({ state: 'visible', timeout: 15_000 });
  }

  const first = samples[0];
  const last = samples.at(-1);
  assert.ok(
    last.Threads <= first.Threads + 4,
    `PTY recovery leaked threads: ${JSON.stringify(samples)}`,
  );
  assert.ok(
    last.Handles <= first.Handles + 24,
    `PTY recovery leaked handles: ${JSON.stringify(samples)}`,
  );
  assert.ok(
    last.PrivateBytes <= first.PrivateBytes + 32 * 1024 * 1024,
    `PTY recovery grew private memory unexpectedly: ${JSON.stringify(samples)}`,
  );

  console.log(JSON.stringify({
    cycles,
    spawnedPtySessions: cycles * 5,
    samples,
    deltas: {
      handles: last.Handles - first.Handles,
      threads: last.Threads - first.Threads,
      privateBytes: last.PrivateBytes - first.PrivateBytes,
    },
  }, null, 2));
} finally {
  await browser?.close();
  child.kill();
}
