const ARABIC =
  '\u0600-\u06FF\u0750-\u077F\u08A0-\u08FF\uFB50-\uFDFF\uFE70-\uFEFF';
const HAS_ARABIC = new RegExp(`[${ARABIC}]`, 'u');
const IS_ARABIC = new RegExp(`^[${ARABIC}]$`, 'u');
const IS_ARABIC_RUN_CHARACTER = new RegExp(
  `^[${ARABIC}\\u200C\\u200D\\s\\p{P}]$`,
  'u',
);
const HAS_NON_ARABIC_RUN_CONTENT = new RegExp(
  `[^${ARABIC}\\u200C\\u200D\\s\\p{P}]`,
  'u',
);
const TRAILING_SENTENCE_PUNCTUATION = new RegExp(
  `^[\\s.,:;!?،؛؟…»”’\\)\\]\\}]+$`,
  'u',
);

/**
 * Returns JavaScript string ranges for xterm's DOM character joiner.
 *
 * The terminal buffer keeps the original Unicode from the PTY. The renderer
 * groups only visible Arabic phrases, allowing the browser's text engine to
 * apply contextual shaping and bidirectional order over the whole current
 * phrase whenever a line changes.
 */
export function findArabicJoinRanges(line: string): [number, number][] {
  const ranges: [number, number][] = [];
  let cursor = 0;

  while (cursor < line.length) {
    const first = String.fromCodePoint(line.codePointAt(cursor)!);
    if (!IS_ARABIC.test(first)) {
      cursor += first.length;
      continue;
    }

    const start = cursor;
    let scan = cursor + first.length;
    let lastArabicEnd = scan;

    while (scan < line.length) {
      const character = String.fromCodePoint(line.codePointAt(scan)!);
      if (!IS_ARABIC_RUN_CHARACTER.test(character)) break;
      scan += character.length;
      if (IS_ARABIC.test(character)) lastArabicEnd = scan;
    }

    // Keep punctuation inside Arabic prose so "مرحبا!" and
    // "كيف حالك؟" render with the mark on the visual left. Do not absorb
    // separators such as a TUI pipe or Markdown dash before a Latin run.
    const tail = line.slice(lastArabicEnd, scan).replace(/\s+$/u, '');
    const end = tail && TRAILING_SENTENCE_PUNCTUATION.test(tail)
      ? lastArabicEnd + tail.length
      : lastArabicEnd;

    if (end - start > 1) ranges.push([start, end]);
    cursor = Math.max(scan, cursor + first.length);
  }

  return ranges;
}

/**
 * Identifies renderer spans that contain Arabic-script content plus neutral
 * whitespace/punctuation. Latin text, digits, symbols, and box drawing are
 * deliberately excluded so command and TUI geometry remains untouched.
 */
export function isArabicOnlyRenderRun(text: string): boolean {
  return HAS_ARABIC.test(text) && !HAS_NON_ARABIC_RUN_CONTENT.test(text);
}
