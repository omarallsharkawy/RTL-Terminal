import type { TerminalFrame, TerminalLine } from './types';

const samples = [
  'مرحبا بك في RTL Terminal',
  'PS E:\\rtl terminal> cd C:\\Users\\Administrator\\مشاريع',
  'git status --short',
  ' M src-tauri/src/bidi.rs',
  '?? src/components/TerminalCanvas.tsx',
  'node --version => v24.15.0',
  'خلط عربي English 123 ومسارات /var/www/app يعمل بدون قلب الكود',
  'npm run tauri:dev',
];

function firstStrongDirection(text: string): 'rtl' | 'ltr' {
  for (const ch of text) {
    if (/[\u0600-\u06ff]/.test(ch)) return 'rtl';
    if (/[A-Za-z]/.test(ch)) return 'ltr';
  }
  return 'ltr';
}

export function makeMockFrame(tick: number, cols = 96, rows = 28): TerminalFrame {
  const visible = samples.slice(0, Math.min(samples.length, 2 + (tick % samples.length)));
  const lines: TerminalLine[] = Array.from({ length: rows }, (_, row) => {
    const text = visible[row] ?? '';
    const padded = text.padEnd(cols, ' ').slice(0, cols);
    return {
      row,
      baseDirection: firstStrongDirection(text),
      cells: [...padded].map((ch, logicalIndex) => ({
        ch,
        logicalIndex,
        width: 1,
        style: {
          fg: row === 0 ? [122, 250, 195] : row > 2 && text.startsWith(' ') ? [153, 179, 198] : [228, 238, 247],
          bold: row === 0,
        },
      })),
    };
  });
  return {
    cols,
    rows,
    cursorCol: (tick * 3) % 44,
    cursorRow: Math.min(visible.length - 1, rows - 1),
    cursorVisible: tick % 2 === 0,
    title: 'PowerShell - demo mode',
    lines,
  };
}
