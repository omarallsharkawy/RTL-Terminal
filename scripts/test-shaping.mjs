import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { findArabicJoinRanges } from '../src/components/arabicRenderer.ts';

let passed = 0;

function expect(name, actual, expected) {
  assert.deepEqual(actual, expected, name);
  passed += 1;
  console.log(`  ✓ ${name}`);
}

function rangesIn(line, fragments) {
  let cursor = 0;
  return fragments.map((fragment) => {
    const start = line.indexOf(fragment, cursor);
    assert.notEqual(start, -1, `missing fixture fragment: ${fragment}`);
    cursor = start + fragment.length;
    return [start, cursor];
  });
}

console.log('Arabic DOM renderer tests');

expect('empty line has no joined ranges', findArabicJoinRanges(''), []);
expect('Latin-only line is untouched', findArabicJoinRanges('git status'), []);
expect('ASCII digits are untouched', findArabicJoinRanges('version 12.5'), []);
expect('one Arabic cell needs no joiner', findArabicJoinRanges('ب'), []);

const word = 'مرحبا';
expect('a complete Arabic word is one render run', findArabicJoinRanges(word), [[0, word.length]]);

const phrase = 'مرحبا بالعالم';
expect('spaces stay inside an Arabic phrase', findArabicJoinRanges(phrase), [[0, phrase.length]]);

const mixed = 'echo مرحبا world 123';
expect(
  'Arabic joins without moving surrounding Latin text',
  findArabicJoinRanges(mixed),
  rangesIn(mixed, ['مرحبا']),
);

const splitMixed = 'مرحبا English بالعالم';
expect(
  'Latin text separates two independent Arabic runs',
  findArabicJoinRanges(splitMixed),
  rangesIn(splitMixed, ['مرحبا', 'بالعالم']),
);

const punctuation = 'مرحبا، بالعالم';
expect(
  'Arabic punctuation remains in the phrase',
  findArabicJoinRanges(punctuation),
  [[0, punctuation.length]],
);

const latinPunctuation = 'مرحبا | English';
expect(
  'ASCII punctuation stays outside the Arabic run',
  findArabicJoinRanges(latinPunctuation),
  rangesIn(latinPunctuation, ['مرحبا']),
);

const diacritics = 'مَرْحَبًا';
expect(
  'combining marks remain in the joined run',
  findArabicJoinRanges(diacritics),
  [[0, diacritics.length]],
);

const zwnj = 'می‌خواهم';
expect(
  'ZWNJ remains inside Persian text',
  findArabicJoinRanges(zwnj),
  [[0, zwnj.length]],
);

const zwj = 'ب‍ب';
expect('ZWJ remains inside Arabic text', findArabicJoinRanges(zwj), [[0, zwj.length]]);

const persian = 'سلام کیف';
expect(
  'Persian extensions join as one phrase',
  findArabicJoinRanges(persian),
  [[0, persian.length]],
);

const presentationForms = 'ﻣﺮﺣﺒﺎ';
expect(
  'presentation forms from legacy applications still join',
  findArabicJoinRanges(presentationForms),
  [[0, presentationForms.length]],
);

const emojiPrefix = '🧪 مرحبا';
expect(
  'ranges use JavaScript UTF-16 indexes expected by xterm',
  findArabicJoinRanges(emojiPrefix),
  rangesIn(emojiPrefix, ['مرحبا']),
);

expect(
  'repeated calls reset the global matcher',
  findArabicJoinRanges('مرحبا'),
  [[0, 'مرحبا'.length]],
);

const productionSource = readFileSync(
  new URL('../src/components/XtermTerminal.tsx', import.meta.url),
  'utf8',
);
assert.match(
  productionSource,
  /registerCharacterJoiner\(findArabicJoinRanges\)/,
  'production terminal must install the tested Arabic joiner',
);
passed += 1;
console.log('  ✓ production terminal installs the tested Arabic joiner');

assert.doesNotMatch(
  productionSource,
  /shapeArabic|createArabicOutputBuffer/,
  'production terminal must not rewrite PTY output into presentation forms',
);
passed += 1;
console.log('  ✓ production terminal keeps PTY output unchanged');

console.log(`\n${passed}/${passed} Arabic renderer checks passed`);
