// Validates Arabic-run detection used by src/components/XtermTerminal.tsx and
// checks a few invariants for the Arabic pre-shaper used before xterm rendering.
// Run: node scripts/test-shaping.mjs

const RLE = '\u202B';
const PDF = '\u202C';
const A = '\u0600-\u06FF\u0750-\u077F\u08A0-\u08FF\uFB50-\uFDFF\uFE70-\uFEFF';
const arabicRun = new RegExp(`[${A}](?:[${A} ]*[${A}])?`, 'g');
const ansiEscape = /(\x1b\][\s\S]*?(?:\x07|\x1b\\)|\x1bP[\s\S]*?\x1b\\|\x1b\^[\s\S]*?\x1b\\|\x1b_[\s\S]*?\x1b\\|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-_])/g;

function runs(line) {
  const out = [];
  arabicRun.lastIndex = 0;
  let m;
  while ((m = arabicRun.exec(line)) !== null) {
    if (m[0].length > 1) out.push([m.index, m.index + m[0].length]);
  }
  return out;
}

function shapeArabicSmoke(input) {
  ansiEscape.lastIndex = 0;
  return input.replace(arabicRun, (run) => `${RLE}${run}${PDF}`);
}

const cases = [
  // label, input, expected ranges [start,end)
  ['single word', 'مرحبا', [[0, 5]]],
  ['two words joined across space', 'مرحبا بك', [[0, 8]]],
  ['arabic between english', 'hi مرحبا bye', [[3, 8]]],
  ['trailing space not included', 'مرحبا ', [[0, 5]]],
  ['lone letter not joined (len 1)', 'a ب c', []],
  ['english only', 'hello world', []],
  ['english+arabic+english words', 'run علي now', [[4, 7]]],
];

const invariantCases = [
  ['wraps Arabic with RTL marks', () => shapeArabicSmoke('hi مرحبا').includes(`${RLE}مرحبا${PDF}`)],
  ['does not wrap English', () => shapeArabicSmoke('hello world') === 'hello world'],
  ['preserves CSI color sequence', () => shapeArabicSmoke('\x1b[31mمرحبا\x1b[0m').startsWith('\x1b[31m')],
];

let pass = 0;
let total = 0;
for (const [label, input, expect] of cases) {
  total++;
  const got = runs(input);
  const ok = JSON.stringify(got) === JSON.stringify(expect);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label.padEnd(34)} got=${JSON.stringify(got)}`);
  if (ok) pass++;
}

for (const [label, check] of invariantCases) {
  total++;
  const ok = Boolean(check());
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`);
  if (ok) pass++;
}

console.log(`\n${pass}/${total} cases passed`);
process.exit(pass === total ? 0 : 1);
