const RLE = '\u202B';
const PDF = '\u202C';

interface ArabicShape {
  isolated: string;
  final?: string;
  initial?: string;
  medial?: string;
}

interface ShapeArabicOptions {
  /**
   * Keep one output code point per input Arabic code point. This is safer for
   * terminal PTY output because xterm cursor/cell math remains predictable.
   */
  preserveCellCount?: boolean;
}

const SHAPES: Record<string, ArabicShape> = {
  '\u0621': { isolated: '\uFE80' },
  '\u0622': { isolated: '\uFE81', final: '\uFE82' },
  '\u0623': { isolated: '\uFE83', final: '\uFE84' },
  '\u0624': { isolated: '\uFE85', final: '\uFE86' },
  '\u0625': { isolated: '\uFE87', final: '\uFE88' },
  '\u0626': { isolated: '\uFE89', final: '\uFE8A', initial: '\uFE8B', medial: '\uFE8C' },
  '\u0627': { isolated: '\uFE8D', final: '\uFE8E' },
  '\u0628': { isolated: '\uFE8F', final: '\uFE90', initial: '\uFE91', medial: '\uFE92' },
  '\u0629': { isolated: '\uFE93', final: '\uFE94' },
  '\u062A': { isolated: '\uFE95', final: '\uFE96', initial: '\uFE97', medial: '\uFE98' },
  '\u062B': { isolated: '\uFE99', final: '\uFE9A', initial: '\uFE9B', medial: '\uFE9C' },
  '\u062C': { isolated: '\uFE9D', final: '\uFE9E', initial: '\uFE9F', medial: '\uFEA0' },
  '\u062D': { isolated: '\uFEA1', final: '\uFEA2', initial: '\uFEA3', medial: '\uFEA4' },
  '\u062E': { isolated: '\uFEA5', final: '\uFEA6', initial: '\uFEA7', medial: '\uFEA8' },
  '\u062F': { isolated: '\uFEA9', final: '\uFEAA' },
  '\u0630': { isolated: '\uFEAB', final: '\uFEAC' },
  '\u0631': { isolated: '\uFEAD', final: '\uFEAE' },
  '\u0632': { isolated: '\uFEAF', final: '\uFEB0' },
  '\u0633': { isolated: '\uFEB1', final: '\uFEB2', initial: '\uFEB3', medial: '\uFEB4' },
  '\u0634': { isolated: '\uFEB5', final: '\uFEB6', initial: '\uFEB7', medial: '\uFEB8' },
  '\u0635': { isolated: '\uFEB9', final: '\uFEBA', initial: '\uFEBB', medial: '\uFEBC' },
  '\u0636': { isolated: '\uFEBD', final: '\uFEBE', initial: '\uFEBF', medial: '\uFEC0' },
  '\u0637': { isolated: '\uFEC1', final: '\uFEC2', initial: '\uFEC3', medial: '\uFEC4' },
  '\u0638': { isolated: '\uFEC5', final: '\uFEC6', initial: '\uFEC7', medial: '\uFEC8' },
  '\u0639': { isolated: '\uFEC9', final: '\uFECA', initial: '\uFECB', medial: '\uFECC' },
  '\u063A': { isolated: '\uFECD', final: '\uFECE', initial: '\uFECF', medial: '\uFED0' },
  '\u0641': { isolated: '\uFED1', final: '\uFED2', initial: '\uFED3', medial: '\uFED4' },
  '\u0642': { isolated: '\uFED5', final: '\uFED6', initial: '\uFED7', medial: '\uFED8' },
  '\u0643': { isolated: '\uFED9', final: '\uFEDA', initial: '\uFEDB', medial: '\uFEDC' },
  '\u0644': { isolated: '\uFEDD', final: '\uFEDE', initial: '\uFEDF', medial: '\uFEE0' },
  '\u0645': { isolated: '\uFEE1', final: '\uFEE2', initial: '\uFEE3', medial: '\uFEE4' },
  '\u0646': { isolated: '\uFEE5', final: '\uFEE6', initial: '\uFEE7', medial: '\uFEE8' },
  '\u0647': { isolated: '\uFEE9', final: '\uFEEA', initial: '\uFEEB', medial: '\uFEEC' },
  '\u0648': { isolated: '\uFEED', final: '\uFEEE' },
  '\u0649': { isolated: '\uFEEF', final: '\uFEF0' },
  '\u064A': { isolated: '\uFEF1', final: '\uFEF2', initial: '\uFEF3', medial: '\uFEF4' },

  // Common Persian/Urdu extensions. Unknown Arabic-script letters pass through.
  '\u067E': { isolated: '\uFB56', final: '\uFB57', initial: '\uFB58', medial: '\uFB59' },
  '\u0686': { isolated: '\uFB7A', final: '\uFB7B', initial: '\uFB7C', medial: '\uFB7D' },
  '\u0698': { isolated: '\uFB8A', final: '\uFB8B' },
  '\u06A9': { isolated: '\uFB8E', final: '\uFB8F', initial: '\uFB90', medial: '\uFB91' },
  '\u06AF': { isolated: '\uFB92', final: '\uFB93', initial: '\uFB94', medial: '\uFB95' },
  '\u06CC': { isolated: '\uFBFC', final: '\uFBFD', initial: '\uFBFE', medial: '\uFBFF' },
};

const LAM_ALEF: Record<string, { isolated: string; final: string }> = {
  '\u0622': { isolated: '\uFEF5', final: '\uFEF6' },
  '\u0623': { isolated: '\uFEF7', final: '\uFEF8' },
  '\u0625': { isolated: '\uFEF9', final: '\uFEFA' },
  '\u0627': { isolated: '\uFEFB', final: '\uFEFC' },
};

const ARABIC_RANGE = '\\u0600-\\u06FF\\u0750-\\u077F\\u08A0-\\u08FF\\uFB50-\\uFDFF\\uFE70-\\uFEFF';
const ARABIC_RUN = new RegExp(`[${ARABIC_RANGE}](?:[${ARABIC_RANGE} \\u200C\\u200D]*[${ARABIC_RANGE}])?`, 'g');
const ANSI_ESCAPE = /(\x1b\][\s\S]*?(?:\x07|\x1b\\)|\x1bP[\s\S]*?\x1b\\|\x1b\^[\s\S]*?\x1b\\|\x1b_[\s\S]*?\x1b\\|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-_])/g;

function isTransparentMark(char: string): boolean {
  return /[\u0610-\u061A\u064B-\u065F\u0670\u06D6-\u06ED]/u.test(char);
}

function canJoinPrevious(char: string): boolean {
  const shape = SHAPES[char];
  return Boolean(shape?.final || shape?.medial);
}

function canJoinNext(char: string): boolean {
  const shape = SHAPES[char];
  return Boolean(shape?.initial || shape?.medial);
}

function previousShapeableIndex(chars: string[], start: number): number {
  for (let i = start - 1; i >= 0; i -= 1) {
    if (isTransparentMark(chars[i])) continue;
    return SHAPES[chars[i]] ? i : -1;
  }
  return -1;
}

function nextShapeableIndex(chars: string[], start: number): number {
  for (let i = start + 1; i < chars.length; i += 1) {
    if (isTransparentMark(chars[i])) continue;
    return SHAPES[chars[i]] ? i : -1;
  }
  return -1;
}

function shapeRun(run: string, preserveCellCount: boolean): string {
  const chars = Array.from(run);
  let output = '';

  for (let i = 0; i < chars.length; i += 1) {
    const char = chars[i];
    const shape = SHAPES[char];

    if (!shape) {
      output += char;
      continue;
    }

    const prev = previousShapeableIndex(chars, i);
    const connectsPrevious = prev >= 0 && canJoinNext(chars[prev]) && canJoinPrevious(char);

    const immediateNext = chars[i + 1];
    const lamAlef = char === '\u0644' && immediateNext ? LAM_ALEF[immediateNext] : undefined;
    if (lamAlef && !preserveCellCount) {
      output += connectsPrevious ? lamAlef.final : lamAlef.isolated;
      i += 1;
      continue;
    }

    const next = nextShapeableIndex(chars, i);
    const connectsNext = next >= 0 && canJoinNext(char) && canJoinPrevious(chars[next]);

    if (connectsPrevious && connectsNext && shape.medial) {
      output += shape.medial;
    } else if (connectsPrevious && shape.final) {
      output += shape.final;
    } else if (connectsNext && shape.initial) {
      output += shape.initial;
    } else {
      output += shape.isolated;
    }
  }

  return output;
}

function shapePlainText(text: string, options: Required<ShapeArabicOptions>): string {
  ARABIC_RUN.lastIndex = 0;
  return text.replace(ARABIC_RUN, (run) => `${RLE}${shapeRun(run, options.preserveCellCount)}${PDF}`);
}

/**
 * Pre-shapes Arabic text for xterm.js renderers that do not run Arabic shaping
 * or full BiDi layout for terminal cells. ANSI escape/control sequences are
 * preserved byte-for-byte, while Arabic runs in printable text are converted to
 * Unicode presentation forms and wrapped in RLE/PDF direction marks.
 */
export function shapeArabic(input: string, options: ShapeArabicOptions = {}): string {
  if (!input) return input;

  const normalizedOptions: Required<ShapeArabicOptions> = {
    preserveCellCount: options.preserveCellCount ?? false,
  };

  ANSI_ESCAPE.lastIndex = 0;
  let output = '';
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = ANSI_ESCAPE.exec(input)) !== null) {
    output += shapePlainText(input.slice(lastIndex, match.index), normalizedOptions);
    output += match[0];
    lastIndex = match.index + match[0].length;
  }

  output += shapePlainText(input.slice(lastIndex), normalizedOptions);
  return output;
}
