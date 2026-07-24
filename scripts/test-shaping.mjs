// Behavior tests for the Arabic pre-shaper used before xterm rendering.
// Imports the real production module (src/components/arabicReshaper.ts) via
// Node 24's native TypeScript type stripping. The shaping tables, run regex,
// and shaping algorithm live only in that module; this file asserts observable
// behavior (expected code points) instead of re-implementing them.
// Run: npm run test:shaping

import { createArabicOutputBuffer, shapeArabic } from '../src/components/arabicReshaper.ts';

const RLE = 0x202b; // RIGHT-TO-LEFT EMBEDDING: opens every shaped run
const PDF = 0x202c; // POP DIRECTIONAL FORMATTING: closes every shaped run
const ZWJ = 0x200d;
const ZWNJ = 0x200c;

const ch = String.fromCharCode;
const cps = (s) => [...s].map((c) => c.codePointAt(0));
const hex = (list) => list.map((cp) => `U+${cp.toString(16).toUpperCase().padStart(4, '0')}`).join(' ');

let pass = 0;
let total = 0;
const failures = [];

function report(label, ok, detail = '') {
  total += 1;
  if (ok) pass += 1;
  else failures.push(label);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}${ok || !detail ? '' : `\n     ${detail}`}`);
}

function expectCps(label, actual, expected) {
  const got = cps(actual);
  const ok = got.length === expected.length && got.every((cp, i) => cp === expected[i]);
  report(label, ok, `got=${hex(got)} want=${hex(expected)}`);
}

function expectValue(label, actual, expected) {
  report(label, Object.is(actual, expected), `got=${JSON.stringify(actual)} want=${JSON.stringify(expected)}`);
}

function expectCond(label, ok) {
  report(label, Boolean(ok));
}

// -- Contextual Arabic forms ------------------------------------------------
expectCps('isolated form', shapeArabic('ب'), [RLE, 0xfe8f, PDF]);
expectCps('initial + final forms', shapeArabic('بب'), [RLE, 0xfe91, 0xfe90, PDF]);
expectCps('initial + medial + final forms', shapeArabic('ببب'), [RLE, 0xfe91, 0xfe92, 0xfe90, PDF]);
expectCps('joins onto right-joining-only alef', shapeArabic('با'), [RLE, 0xfe91, 0xfe8e, PDF]);
expectCps('no join from non-joining alef', shapeArabic('اب'), [RLE, 0xfe8d, 0xfe8f, PDF]);
expectCps('full word contextual chain (مرحبا)', shapeArabic('مرحبا'), [RLE, 0xfee3, 0xfeae, 0xfea3, 0xfe92, 0xfe8e, PDF]);

// -- Transparent marks (harakat) --------------------------------------------
expectCps('fatha is transparent to joining', shapeArabic(`ب${ch(0x064e)}ب`), [RLE, 0xfe91, 0x064e, 0xfe90, PDF]);

// -- ZWJ / ZWNJ -------------------------------------------------------------
expectCps('ZWJ keeps letters joined', shapeArabic(`ب${ch(ZWJ)}ب`), [RLE, 0xfe91, ZWJ, 0xfe90, PDF]);
expectCps('ZWNJ breaks joining', shapeArabic(`ب${ch(ZWNJ)}ب`), [RLE, 0xfe8f, ZWNJ, 0xfe8f, PDF]);
expectCps('ZWNJ inside Persian word (می‌خواهم)', shapeArabic(`می${ch(ZWNJ)}خواهم`), [RLE, 0xfee3, 0xfbfd, ZWNJ, 0xfea7, 0xfeee, 0xfe8d, 0xfeeb, 0xfee2, PDF]);

// -- Supported Persian letters ----------------------------------------------
expectCps('peh joins as initial (پب)', shapeArabic('پب'), [RLE, 0xfb58, 0xfe90, PDF]);
expectCps('cheh isolated (چ)', shapeArabic('چ'), [RLE, 0xfb7a, PDF]);
expectCps('jeh isolated (ژ)', shapeArabic('ژ'), [RLE, 0xfb8a, PDF]);
expectCps('gaf joins as initial (گل)', shapeArabic('گل'), [RLE, 0xfb94, 0xfede, PDF]);
expectCps('keheh + farsi yeh join (کیف)', shapeArabic('کیف'), [RLE, 0xfb90, 0xfbff, 0xfed2, PDF]);

// -- Mixed Arabic / English / digits / punctuation --------------------------
expectCps(
  'english, digits and punctuation pass through',
  shapeArabic('price سعر 12.5 (USD)!'),
  [...cps('price '), RLE, 0xfeb3, 0xfecc, 0xfeae, PDF, ...cps(' 12.5 (USD)!')],
);

// -- Lam-alef ligature ------------------------------------------------------
expectCps('lam-alef ligates by default (لا)', shapeArabic('لا'), [RLE, 0xfefb, PDF]);
expectCps('lam-alef with hamza ligates (لأ)', shapeArabic('لأ'), [RLE, 0xfef7, PDF]);
expectCps('lam-alef with madda ligates (لآ)', shapeArabic('لآ'), [RLE, 0xfef5, PDF]);
expectCps('joined lam-alef uses final ligature (بلا)', shapeArabic('بلا'), [RLE, 0xfe91, 0xfefc, PDF]);
expectCps('preserveCellCount keeps lam and alef separate', shapeArabic('لا', { preserveCellCount: true }), [RLE, 0xfedf, 0xfe8e, PDF]);
expectCps('preserveCellCount shapes joined lam medially', shapeArabic('بلا', { preserveCellCount: true }), [RLE, 0xfe91, 0xfee0, 0xfe8e, PDF]);

// -- ANSI escape preservation -----------------------------------------------
const SHAPED_MRHABA = [0xfee3, 0xfeae, 0xfea3, 0xfe92, 0xfe8e];
expectCps(
  'CSI sequences preserved byte-for-byte',
  shapeArabic('\x1b[1;31mمرحبا\x1b[0m'),
  [...cps('\x1b[1;31m'), RLE, ...SHAPED_MRHABA, PDF, ...cps('\x1b[0m')],
);
expectCps(
  'OSC sequence (BEL terminated) preserved',
  shapeArabic('\x1b]0;my title\x07مرحبا'),
  [...cps('\x1b]0;my title\x07'), RLE, ...SHAPED_MRHABA, PDF],
);
expectCps(
  'OSC sequence (ST terminated) preserved',
  shapeArabic('\x1b]8;;http://example.com\x1b\\مرحبا'),
  [...cps('\x1b]8;;http://example.com\x1b\\'), RLE, ...SHAPED_MRHABA, PDF],
);

// -- Empty / English-only input ---------------------------------------------
expectValue('empty string returns empty', shapeArabic(''), '');
expectValue('english-only input unchanged', shapeArabic('hello world'), 'hello world');
expectValue('digits/punctuation-only input unchanged', shapeArabic('route 66 (ok)!'), 'route 66 (ok)!');

// -- preserveCellCount invariant --------------------------------------------
// Exactly one presentation-form code point per input Arabic letter; only
// zero-width direction controls (ZWJ/ZWNJ) may ride along inside the run.
function presentationFormCount(s) {
  return cps(s).filter((cp) => (cp >= 0xfb50 && cp <= 0xfdff) || (cp >= 0xfe70 && cp <= 0xfeff)).length;
}

expectCond(
  'preserveCellCount: one glyph per letter (مرحبا, 5 letters)',
  presentationFormCount(shapeArabic('مرحبا', { preserveCellCount: true })) === 5,
);
expectCond(
  'default mode merges lam-alef (سلام, 4 letters -> 3 glyphs)',
  presentationFormCount(shapeArabic('سلام')) === 3,
);
expectCond(
  'preserveCellCount: no lam-alef merge (سلام, 4 letters -> 4 glyphs)',
  presentationFormCount(shapeArabic('سلام', { preserveCellCount: true })) === 4,
);
expectCond(
  'preserveCellCount: ZWJ passes through without adding glyphs',
  (() => {
    const out = shapeArabic(`ب${ch(ZWJ)}ب`, { preserveCellCount: true });
    return presentationFormCount(out) === 2 && cps(out).filter((cp) => cp === ZWJ).length === 1;
  })(),
);
expectCond(
  'preserveCellCount: ZWNJ passes through without adding glyphs',
  (() => {
    const out = shapeArabic(`می${ch(ZWNJ)}خواهم`, { preserveCellCount: true });
    return presentationFormCount(out) === 7 && cps(out).filter((cp) => cp === ZWNJ).length === 1;
  })(),
);

// -- Streaming/chunk boundaries ---------------------------------------------
function stream(chunks, options = { preserveCellCount: true }) {
  const buffer = createArabicOutputBuffer(options);
  let output = '';
  for (const chunk of chunks) output += buffer.push(chunk);
  output += buffer.flush();
  return output;
}

expectValue(
  'stream: Arabic word is invariant across chunks',
  stream(['مر', 'ح', 'با']),
  shapeArabic('مرحبا', { preserveCellCount: true }),
);
expectValue(
  'stream: mixed text is invariant across chunks',
  stream(['run مر', 'حبا 12', ' ok']),
  shapeArabic('run مرحبا 12 ok', { preserveCellCount: true }),
);
expectValue(
  'stream: split CSI is preserved and Arabic is shaped',
  stream(['\x1b[1;', '31mمر', 'حبا\x1b[', '0m']),
  shapeArabic('\x1b[1;31mمرحبا\x1b[0m', { preserveCellCount: true }),
);
expectValue(
  'stream: split OSC payload is never shaped',
  stream(['\x1b]0;عنوان', ' عربي', '\x07مر', 'حبا']),
  shapeArabic('\x1b]0;عنوان عربي\x07مرحبا', { preserveCellCount: true }),
);
expectValue(
  'stream: timer flush keeps an incomplete CSI for the next chunk',
  (() => {
    const buffer = createArabicOutputBuffer({ preserveCellCount: true });
    let output = buffer.push('\x1b[31');
    output += buffer.flush();
    const pendingAfterTimer = buffer.hasPending;
    output += buffer.push('mمرحبا');
    output += buffer.flush();
    return `${pendingAfterTimer}|${output}`;
  })(),
  `true|${shapeArabic('\x1b[31mمرحبا', { preserveCellCount: true })}`,
);
expectValue(
  'stream: timer flush releases Arabic before an incomplete control',
  (() => {
    const buffer = createArabicOutputBuffer({ preserveCellCount: true });
    let output = buffer.push('مرحبا\x1b[');
    output += buffer.flush();
    output += buffer.push('0m done');
    output += buffer.flush();
    return output;
  })(),
  shapeArabic('مرحبا', { preserveCellCount: true }) + '\x1b[0m done',
);
expectValue(
  'stream: English output remains immediate',
  (() => {
    const buffer = createArabicOutputBuffer({ preserveCellCount: true });
    const output = buffer.push('plain output');
    return `${output}|${buffer.hasPending}`;
  })(),
  'plain output|false',
);
expectValue(
  'stream: flush releases a final Arabic prompt',
  (() => {
    const buffer = createArabicOutputBuffer({ preserveCellCount: true });
    const immediate = buffer.push('اسم المستخدم ');
    return `${immediate}|${buffer.hasPending}|${buffer.flush()}`;
  })(),
  `|true|${shapeArabic('اسم المستخدم ', { preserveCellCount: true })}`,
);
expectValue(
  'stream: reset drops pending text between sessions',
  (() => {
    const buffer = createArabicOutputBuffer({ preserveCellCount: true });
    buffer.push('قديم');
    buffer.reset();
    return buffer.push('new session') + buffer.flush();
  })(),
  'new session',
);

console.log(`\n${pass}/${total} cases passed`);
if (failures.length > 0) console.log(`failed: ${failures.join(', ')}`);
process.exit(pass === total ? 0 : 1);
