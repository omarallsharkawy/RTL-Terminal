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
const ARABIC_SCRIPT_CHARACTER = /\p{Script=Arabic}/u;
const LATIN_OR_NUMBER_CHARACTER = /[\p{Script=Latin}\p{Number}]/u;
const FIRST_STRONG_CHARACTER = /[\p{Script=Arabic}\p{Script=Latin}\p{Number}]/u;
const FIXED_LTR_LINE_PREFIX = /^(?:PS\s+[A-Za-z]:\\|[A-Za-z]:\\|[>$❯]\s)/u;
const ARABIC_GRAPHEME_SEGMENTER = new Intl.Segmenter('ar', { granularity: 'grapheme' });

function reverseGraphemes(text: string): string {
  return Array.from(
    ARABIC_GRAPHEME_SEGMENTER.segment(text),
    ({ segment }) => segment,
  ).reverse().join('');
}

function countCodePoints(text: string, matcher: RegExp): number {
  let count = 0;
  for (const character of text) {
    if (matcher.test(character)) count += 1;
  }
  return count;
}

function firstStrongCharacter(text: string): string | undefined {
  for (const character of text) {
    if (FIRST_STRONG_CHARACTER.test(character)) return character;
  }
  return undefined;
}

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

/**
 * Identifies renderer spans that contain neutral whitespace or punctuation,
 * but no Arabic script and no Latin/code/number/box-drawing content.
 */
export function isNeutralRenderRun(text: string): boolean {
  return (
    text.length > 0 &&
    !HAS_ARABIC.test(text) &&
    !HAS_NON_ARABIC_RUN_CONTENT.test(text)
  );
}

/**
 * Some full-screen TUIs pre-apply BiDi visual ordering before painting their
 * input cells. Match those visual Arabic cells against the exact unsent input
 * Twitty observed, then return the original logical run for DOM-only display.
 * PTY and xterm buffer data remain untouched.
 */
export function logicalArabicRunForVisualText(
  visualText: string,
  pendingInput: string,
): string | null {
  for (const [start, end] of findArabicJoinRanges(pendingInput)) {
    const logicalRun = pendingInput.slice(start, end);
    if (
      logicalRun !== visualText
      && reverseGraphemes(logicalRun) === visualText
    ) {
      return logicalRun;
    }
  }
  return null;
}

/**
 * Groups contiguous renderer spans that form a single local RTL visual phrase.
 * A group begins and ends with an Arabic-only span, and may include neutral
 * whitespace/punctuation spans between them.
 */
export function findArabicRenderGroups<T>(
  items: T[],
  isArabic: (item: T) => boolean,
  isNeutral: (item: T) => boolean,
): T[][] {
  const groups: T[][] = [];
  let index = 0;

  while (index < items.length) {
    if (!isArabic(items[index])) {
      index += 1;
      continue;
    }

    const currentGroup: T[] = [items[index]];
    let lastArabicIdx = index;
    let scan = index + 1;

    while (scan < items.length) {
      const item = items[scan];
      if (isArabic(item)) {
        currentGroup.push(item);
        lastArabicIdx = scan;
        scan += 1;
      } else if (isNeutral(item)) {
        currentGroup.push(item);
        scan += 1;
      } else {
        break;
      }
    }

    const validLength = lastArabicIdx - index + 1;
    groups.push(currentGroup.slice(0, validLength));
    index = lastArabicIdx + 1;
  }

  return groups;
}

/**
 * Keeps an active terminal cursor inside the adjacent RTL visual group. xterm
 * represents the cursor as another renderer span. If it remains outside the
 * group, ANSI-split Arabic words reorder correctly but the caret lands on the
 * visual right instead of following the logical end on the visual left.
 */
export function includeAdjacentCursorInRenderGroups<T>(
  groups: T[][],
  items: T[],
  isCursor: (item: T) => boolean,
  isNeutral: (item: T) => boolean,
): T[][] {
  return groups.map((group) => {
    if (group.some(isCursor)) return group;

    const start = items.indexOf(group[0]);
    const end = items.indexOf(group[group.length - 1]);
    if (start < 0 || end < start) return group;

    for (let scan = end + 1; scan < items.length; scan += 1) {
      const item = items[scan];
      if (isCursor(item)) return [...group, ...items.slice(end + 1, scan + 1)];
      if (!isNeutral(item)) break;
    }

    for (let scan = start - 1; scan >= 0; scan -= 1) {
      const item = items[scan];
      if (isCursor(item)) return [...items.slice(scan, start), ...group];
      if (!isNeutral(item)) break;
    }

    return group;
  });
}

/**
 * Arabic prose needs an RTL paragraph base so embedded English/code stays in
 * the correct reading position. Keep shell prompts and the active cursor row
 * anchored to the terminal's LTR grid; their local Arabic render groups are
 * still reordered independently by the DOM decorator.
 */
export function shouldRenderLineRtl(
  text: string,
  hasActiveCursor = false,
): boolean {
  if (hasActiveCursor || FIXED_LTR_LINE_PREFIX.test(text)) return false;

  const arabicCount = countCodePoints(text, ARABIC_SCRIPT_CHARACTER);
  const latinOrNumberCount = countCodePoints(text, LATIN_OR_NUMBER_CHARACTER);
  const firstStrong = firstStrongCharacter(text);
  if (firstStrong && HAS_ARABIC.test(firstStrong)) return true;

  return arabicCount >= 6 && arabicCount > latinOrNumberCount * 1.15;
}
