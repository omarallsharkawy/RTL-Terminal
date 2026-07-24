const ARABIC =
  '\u0600-\u06FF\u0750-\u077F\u08A0-\u08FF\uFB50-\uFDFF\uFE70-\uFEFF';

// Join complete Arabic phrases, including spaces and Arabic combining marks,
// but stop at Latin text and ASCII digits so mixed command lines retain normal
// terminal cell positions around their LTR segments.
const ARABIC_PHRASE = new RegExp(
  `[${ARABIC}](?:[${ARABIC}\\u200C\\u200D ]*[${ARABIC}])?`,
  'g',
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
  ARABIC_PHRASE.lastIndex = 0;

  let match: RegExpExecArray | null;
  while ((match = ARABIC_PHRASE.exec(line)) !== null) {
    if (match[0].length > 1) {
      ranges.push([match.index, match.index + match[0].length]);
    }
  }

  return ranges;
}
