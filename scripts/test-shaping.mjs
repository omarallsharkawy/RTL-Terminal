// Validates the Arabic-run detection used by the WebGL character joiner in
// src/components/XtermTerminal.tsx. Shaping + RTL ordering itself is performed by
// the browser's canvas text engine at render time (not unit-testable in Node), so
// this test only asserts which cell ranges get handed to the joiner: each range
// must start and end on an Arabic letter and may span spaces between Arabic words.
// Run: node scripts/test-shaping.mjs

const A = '\u0600-\u06FF\u0750-\u077F\u08A0-\u08FF\uFB50-\uFDFF\uFE70-\uFEFF';
const arabicRun = new RegExp(`[${A}](?:[${A} ]*[${A}])?`, 'g');

function runs(line) {
  const out = [];
  arabicRun.lastIndex = 0;
  let m;
  while ((m = arabicRun.exec(line)) !== null) {
    if (m[0].length > 1) out.push([m.index, m.index + m[0].length]);
  }
  return out;
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

let pass = 0;
for (const [label, input, expect] of cases) {
  const got = runs(input);
  const ok = JSON.stringify(got) === JSON.stringify(expect);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label.padEnd(34)} got=${JSON.stringify(got)}`);
  if (ok) pass++;
}
console.log(`\n${pass}/${cases.length} cases passed`);
process.exit(pass === cases.length ? 0 : 1);
