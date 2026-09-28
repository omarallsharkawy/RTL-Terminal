const graphemeSegmenter = new Intl.Segmenter('ar', { granularity: 'grapheme' });

export function splitGraphemes(text: string): string[] {
  return Array.from(graphemeSegmenter.segment(text), ({ segment }) => segment);
}

export function reverseGraphemes(text: string): string {
  return splitGraphemes(text).reverse().join('');
}

/**
 * Mirrors the current unsent terminal input without intercepting or changing
 * PTY data. It intentionally handles only reliable append/delete/clear input;
 * cursor-navigation escape sequences are ignored until the TUI repaints.
 */
export function updatePendingTerminalInput(current: string, chunk: string): string {
  if (chunk.startsWith('\x1b')) return current;

  let next = current;
  for (const character of chunk) {
    if (character === '\r' || character === '\n' || character === '\x03' || character === '\x15') {
      next = '';
    } else if (character === '\x7f' || character === '\b') {
      const graphemes = splitGraphemes(next);
      graphemes.pop();
      next = graphemes.join('');
    } else if (character >= ' ') {
      next += character;
    }
  }

  return next.slice(-4096);
}
