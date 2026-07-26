import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  findArabicJoinRanges,
  findArabicRenderGroups,
  includeAdjacentCursorInRenderGroups,
  isArabicOnlyRenderRun,
  isNeutralRenderRun,
  logicalArabicRunForVisualText,
  shouldRenderLineRtl,
} from '../src/components/arabicRenderer.ts';
import { updatePendingTerminalInput } from '../src/components/terminalInputMirror.ts';

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

const asciiSentence = 'مرحبا! كيف حالك؟';
expect(
  'sentence punctuation stays visually attached to Arabic prose',
  findArabicJoinRanges(asciiSentence),
  [[0, asciiSentence.length]],
);

const parenthesized = 'يعمل (الآن)، بالتأكيد.';
expect(
  'parentheses and trailing punctuation stay inside Arabic prose',
  findArabicJoinRanges(parenthesized),
  [[0, parenthesized.length]],
);

const latinPunctuation = 'مرحبا | English';
expect(
  'ASCII punctuation stays outside the Arabic run',
  findArabicJoinRanges(latinPunctuation),
  rangesIn(latinPunctuation, ['مرحبا']),
);

const markdownSeparator = 'مرحبا - English';
expect(
  'Markdown separators before Latin text stay outside the Arabic run',
  findArabicJoinRanges(markdownSeparator),
  rangesIn(markdownSeparator, ['مرحبا']),
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

expect('Arabic-only renderer span is detected', isArabicOnlyRenderRun('مرحبا بالعالم'), true);
expect('Arabic diacritics remain eligible for compact layout', isArabicOnlyRenderRun('مَرْحَبًا'), true);
expect('Arabic punctuation remains eligible for RTL layout', isArabicOnlyRenderRun('مرحبًا!'), true);
expect('Latin text is never compacted', isArabicOnlyRenderRun('Fix tests 123'), false);
expect('mixed Arabic and Latin span is never compacted', isArabicOnlyRenderRun('مرحبا Fix'), false);
expect('neutral whitespace span is detected', isNeutralRenderRun(' '), true);
expect('neutral punctuation span is detected', isNeutralRenderRun(', '), true);
expect('Latin span is not neutral', isNeutralRenderRun('Code123'), false);
expect('box drawing span is not neutral', isNeutralRenderRun('│'), false);

expect(
  'contiguous Arabic spans group across neutral spans',
  findArabicRenderGroups(
    ['مرحبا', ' ', 'بالعالم', ' | ', 'English'],
    isArabicOnlyRenderRun,
    isNeutralRenderRun,
  ),
  [['مرحبا', ' ', 'بالعالم']],
);

expect(
  'TUI input groups Arabic words split by prompt colors',
  findArabicRenderGroups(
    ['❯', ' ', 'مرحبا', ' ', 'بالعالم', ' '],
    isArabicOnlyRenderRun,
    isNeutralRenderRun,
  ),
  [['مرحبا', ' ', 'بالعالم']],
);

const tuiCursorAtEnd = [
  { text: '❯', cursor: false },
  { text: ' ', cursor: false },
  { text: 'مرحبا', cursor: false },
  { text: ' ', cursor: false },
  { text: 'بالعالم', cursor: false },
  { text: ' ', cursor: true },
];
const tuiCursorBaseGroup = findArabicRenderGroups(
  tuiCursorAtEnd,
  (item) => isArabicOnlyRenderRun(item.text),
  (item) => isNeutralRenderRun(item.text),
);
expect(
  'TUI cursor follows the logical end of an ANSI-split Arabic phrase',
  includeAdjacentCursorInRenderGroups(
    tuiCursorBaseGroup,
    tuiCursorAtEnd,
    (item) => item.cursor,
    (item) => isNeutralRenderRun(item.text),
  ),
  [[...tuiCursorAtEnd.slice(2)]],
);

const tuiCursorAtStart = [
  { text: '❯', cursor: false },
  { text: ' ', cursor: false },
  { text: ' ', cursor: true },
  { text: 'مرحبا', cursor: false },
  { text: ' ', cursor: false },
  { text: 'بالعالم', cursor: false },
];
const tuiCursorStartGroup = findArabicRenderGroups(
  tuiCursorAtStart,
  (item) => isArabicOnlyRenderRun(item.text),
  (item) => isNeutralRenderRun(item.text),
);
expect(
  'TUI cursor stays at the logical start of an Arabic phrase',
  includeAdjacentCursorInRenderGroups(
    tuiCursorStartGroup,
    tuiCursorAtStart,
    (item) => item.cursor,
    (item) => isNeutralRenderRun(item.text),
  ),
  [[...tuiCursorAtStart.slice(2)]],
);

expect(
  'pre-visualized TUI Arabic is restored from exact pending input',
  logicalArabicRunForVisualText('ملاعلاب ابحرم', 'مرحبا بالعالم'),
  'مرحبا بالعالم',
);
expect(
  'mixed TUI Arabic runs are restored without touching embedded English',
  [
    logicalArabicRunForVisualText('ملاعلاب', 'مرحبا hello بالعالم'),
    logicalArabicRunForVisualText('ابحرم', 'مرحبا hello بالعالم'),
  ],
  ['بالعالم', 'مرحبا'],
);
expect(
  'already-logical Arabic is never reversed',
  logicalArabicRunForVisualText('مرحبا بالعالم', 'مرحبا بالعالم'),
  null,
);
expect(
  'pending input mirror preserves Arabic typing and backspace',
  updatePendingTerminalInput('مرحبا بالعالم', '\x7f!'),
  'مرحبا بالعال!',
);
expect(
  'pending input mirror clears submitted input',
  updatePendingTerminalInput('مرحبا بالعالم', '\r'),
  '',
);

expect(
  'Arabic-dominant prose receives an RTL paragraph base',
  shouldRenderLineRtl('agy هو الواجهة السطرية لمنصة Google Antigravity والمساعدة البرمجية'),
  true,
);
expect(
  'Arabic-led mixed output stays RTL even when the English segment is longer',
  shouldRenderLineRtl("أهلاً وسهلاً! I'm doing great, thank you for asking! 😊"),
  true,
);
expect(
  'active input rows remain on the LTR terminal grid',
  shouldRenderLineRtl('أهلاً وسهلاً بك، كيف أساعدك؟', true),
  false,
);
expect(
  'PowerShell command rows never move to the opposite edge',
  shouldRenderLineRtl('PS C:\\Users\\Administrator> Write-Output "مرحبا بالعالم"'),
  false,
);
expect(
  'Latin-dominant mixed output remains LTR',
  shouldRenderLineRtl('Build passed in Twitty with رسالة قصيرة'),
  false,
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

const productionStyles = readFileSync(
  new URL('../src/styles.css', import.meta.url),
  'utf8',
);
assert.match(
  productionStyles,
  /\.xterm-arabic-run[\s\S]*unicode-bidi:\s*isolate/,
  'Arabic spans must not reorder surrounding Latin cells',
);
passed += 1;
console.log('  ✓ Arabic spans isolate surrounding Latin cells');

assert.match(
  productionSource,
  /shouldRenderLineRtl\(/,
  'production terminal must choose a paragraph base for Arabic-dominant prose',
);
passed += 1;
console.log('  ✓ production terminal assigns Arabic prose an RTL paragraph base');

assert.doesNotMatch(
  productionSource,
  /if\s*\(hasCursor\)\s*(?:return|unwrapArabicGroups)/,
  'cursor rows must still group Arabic spans emitted by arbitrary TUIs',
);
passed += 1;
console.log('  ✓ active TUI input groups Arabic independently of the CLI');

assert.match(
  productionSource,
  /requestAnimationFrame\([\s\S]*pendingArabicRows/,
  'Arabic DOM mutations must be coalesced into one layout pass per frame',
);
passed += 1;
console.log('  ✓ Arabic DOM mutation bursts are coalesced per frame');

assert.doesNotMatch(
  productionSource,
  /shapeArabic|createArabicOutputBuffer/,
  'production terminal must not rewrite PTY output into presentation forms',
);
passed += 1;
console.log('  ✓ production terminal keeps PTY output unchanged');

console.log(`\n${passed}/${passed} Arabic renderer checks passed`);
