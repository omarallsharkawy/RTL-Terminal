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

const result = await page.evaluate(async () => {
  const arabic = /[\u0600-\u06ff\u0750-\u077f\u08a0-\u08ff]/;
  const rows = Array.from(document.querySelectorAll('.xterm-rows > div'));
  const row = rows.find((candidate) => candidate.textContent?.includes('English stays LTR'));
  if (!row) throw new Error('Mixed Arabic demo row was not rendered');
  const mixedProseRow = rows.find(
    (candidate) => candidate.textContent?.includes('Google Antigravity')
      && candidate.textContent?.includes('هو الواجهة'),
  );
  if (!mixedProseRow) throw new Error('Arabic-dominant mixed prose row was not rendered');
  const ansiSplitRow = rows.find(
    (candidate) => candidate.textContent?.includes('ANSI split:'),
  );
  if (!ansiSplitRow) throw new Error('ANSI-split Arabic demo row was not rendered');
  const arabicLedMixedRow = rows.find(
    (candidate) => candidate.textContent?.includes("I'm doing great"),
  );
  if (!arabicLedMixedRow) throw new Error('Arabic-led mixed output row was not rendered');
  const activeMixedRow = rows.find(
    (candidate) => candidate.textContent?.includes('كيفك how are you كويس؟'),
  );
  if (!activeMixedRow) throw new Error('Active mixed-language input row was not rendered');

  let postLayoutMutations = 0;
  const stabilityObserver = new MutationObserver(
    (mutations) => {
      postLayoutMutations += mutations.filter(
        (mutation) => mutation.type === 'childList',
      ).length;
    },
  );
  stabilityObserver.observe(ansiSplitRow, { childList: true, subtree: true });
  await new Promise((resolve) => setTimeout(resolve, 250));
  stabilityObserver.disconnect();

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
    mixedProse: {
      text: mixedProseRow.textContent,
      className: mixedProseRow.className,
      direction: getComputedStyle(mixedProseRow).direction,
      spans: Array.from(mixedProseRow.querySelectorAll('span:not(.xterm-arabic-group)')).map((span) => ({
        text: span.textContent,
        direction: getComputedStyle(span).direction,
        left: span.getBoundingClientRect().left,
        right: span.getBoundingClientRect().right,
      })),
    },
    ansiSplit: {
      text: ansiSplitRow.textContent,
      postLayoutMutations,
      wrappers: ansiSplitRow.querySelectorAll(':scope > .xterm-arabic-group').length,
      spans: Array.from(ansiSplitRow.querySelectorAll('.xterm-arabic-group > span')).map(
        (span) => {
          const rect = span.getBoundingClientRect();
          return {
            text: span.textContent,
            color: getComputedStyle(span).color,
            left: rect.left,
            right: rect.right,
          };
        },
      ),
    },
    arabicLedMixed: {
      text: arabicLedMixedRow.textContent,
      className: arabicLedMixedRow.className,
      direction: getComputedStyle(arabicLedMixedRow).direction,
      spans: Array.from(
        arabicLedMixedRow.querySelectorAll('span:not(.xterm-arabic-group)'),
      ).map((span) => {
        const rect = span.getBoundingClientRect();
        return {
          text: span.textContent,
          direction: getComputedStyle(span).direction,
          left: rect.left,
          right: rect.right,
        };
      }),
    },
    activeMixedInput: {
      text: activeMixedRow.textContent,
      className: activeMixedRow.className,
      hasCursor: Boolean(activeMixedRow.querySelector('.xterm-cursor')),
      wrappers: activeMixedRow.querySelectorAll('.xterm-arabic-group').length,
      arabicRuns: activeMixedRow.querySelectorAll('.xterm-arabic-run').length,
    },
    spans: Array.from(row.querySelectorAll('span.xterm-arabic-run'))
      .filter((span) => arabic.test(span.textContent || ''))
      .map((span) => {
        const style = getComputedStyle(span);
        const text = span.textContent || '';
        const words = [];
        let offset = 0;
        for (const word of text.split(' ')) {
          const start = text.indexOf(word, offset);
          const range = document.createRange();
          range.setStart(span.firstChild, start);
          range.setEnd(span.firstChild, start + word.length);
          const bounds = range.getBoundingClientRect();
          words.push({ left: bounds.left, right: bounds.right });
          offset = start + word.length + 1;
        }
        const visualWords = words.sort((a, b) => a.left - b.left);
        const wordGaps = visualWords.slice(1).map(
          (word, index) => word.left - visualWords[index].right,
        );
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
          wordGaps,
        };
      }),
  };
});

console.log(JSON.stringify(result, null, 2));
assert.equal(result.overlayDisplay, 'none', 'runtime error overlay must stay hidden after mount');
assert.ok(result.rootChildren > 0, 'React root must remain mounted');
assert.ok(result.spans.length > 0, 'Arabic renderer span must exist');
assert.match(
  result.mixedProse.className,
  /xterm-rtl-line/,
  'Arabic-dominant mixed prose must receive the RTL paragraph class',
);
assert.equal(
  result.mixedProse.direction,
  'rtl',
  'Arabic-dominant mixed prose must use an RTL paragraph base',
);
const firstLatin = result.mixedProse.spans.find((span) => span.text.includes('agy'));
const lastLatin = result.mixedProse.spans.find((span) => span.text.includes('Terminal'));
assert.ok(
  firstLatin.left > lastLatin.left,
  'the first logical Latin token must stay at the visual right of RTL prose',
);
for (const latin of result.mixedProse.spans.filter((span) => /[A-Za-z]/.test(span.text))) {
  assert.equal(latin.direction, 'ltr', `Latin token must remain LTR: ${latin.text}`);
}
assert.equal(
  result.ansiSplit.postLayoutMutations,
  0,
  'Arabic grouping must settle instead of triggering a MutationObserver rebuild loop',
);
assert.equal(result.ansiSplit.wrappers, 1, 'ANSI-split Arabic must use one local RTL group');
assert.ok(
  !result.ansiSplit.text.includes('\uFFFD'),
  'valid emoji output must not contain a replacement character',
);
const firstStyledArabic = result.ansiSplit.spans.find((span) => span.text === 'مرحبا');
const secondStyledArabic = result.ansiSplit.spans.find((span) => span.text === 'بالعالم');
assert.ok(
  firstStyledArabic && secondStyledArabic,
  `ANSI-split Arabic leaf spans are missing: ${JSON.stringify(result.ansiSplit.spans)}`,
);
assert.ok(
  firstStyledArabic.left > secondStyledArabic.left,
  'the first logical Arabic word must render on the visual right across ANSI spans',
);
assert.notEqual(
  firstStyledArabic.color,
  secondStyledArabic.color,
  'ANSI-split Arabic spans must retain their distinct colors',
);
assert.match(
  result.arabicLedMixed.className,
  /xterm-rtl-line/,
  'output whose first strong character is Arabic must receive an RTL paragraph base',
);
assert.equal(result.arabicLedMixed.direction, 'rtl');
const arabicGreeting = result.arabicLedMixed.spans.find(
  (span) => span.text.includes('أهلاً'),
);
const englishReply = result.arabicLedMixed.spans.find(
  (span) => span.text.includes("I'm doing"),
);
assert.ok(arabicGreeting && englishReply, 'Arabic-led mixed output spans are missing');
assert.ok(
  arabicGreeting.left > englishReply.left,
  'the first Arabic segment must sit to the visual right of embedded English',
);
assert.equal(englishReply.direction, 'ltr', 'embedded English must remain internally LTR');
assert.equal(
  result.activeMixedInput.hasCursor,
  true,
  'mixed-language input probe must exercise the active cursor row',
);
assert.doesNotMatch(
  result.activeMixedInput.className,
  /xterm-rtl-line/,
  'active mixed-language input must stay on xterm’s LTR grid',
);
assert.equal(
  result.activeMixedInput.wrappers,
  0,
  'active input must not move terminal cells into an RTL wrapper',
);
assert.ok(
  result.activeMixedInput.arabicRuns >= 2,
  'active input must still shape and compact every Arabic run',
);
for (const span of result.spans) {
  assert.match(span.className, /xterm-arabic-run/, 'Arabic span must receive layout class');
  assert.equal(span.direction, 'rtl', 'Arabic span must receive RTL direction');
  assert.equal(
    span.unicodeBidi,
    'isolate',
    'Arabic runs must not reorder surrounding English terminal cells',
  );
  const wordSpacing = parseFloat(span.wordSpacing);
  assert.ok(
    wordSpacing < -3 && wordSpacing > -4.5,
    'Arabic word spacing must remove only the excess monospace cell width',
  );
  assert.ok(
    span.letterSpacing === 'normal' || span.letterSpacing === '0px',
    'xterm ligature compensation must not spread Arabic cursive text',
  );
  assert.ok(
    span.wordGaps.every((gap) => gap >= 3 && gap <= 7),
    `Arabic visual word gaps must stay between 3px and 7px: ${span.wordGaps}`,
  );
}
await browser.close();
server.close();
