import { useEffect, useRef } from 'react';
import type { TerminalFrame, ThemeName, CellStyle } from '../lib/types';

const themes: Record<ThemeName, { bg: string; grid: string; cursor: string; selection: string }> = {
  midnight: { bg: '#071013', grid: 'rgba(117, 225, 255, 0.06)', cursor: '#7afac3', selection: 'rgba(122, 250, 195, 0.18)' },
  emerald: { bg: '#06130f', grid: 'rgba(92, 255, 184, 0.07)', cursor: '#6ee7ff', selection: 'rgba(110, 231, 255, 0.16)' },
  graphite: { bg: '#101214', grid: 'rgba(239, 246, 255, 0.06)', cursor: '#f5c542', selection: 'rgba(245, 197, 66, 0.16)' },
};

function rgb(value?: [number, number, number], fallback = '#e4eef7') {
  return value ? `rgb(${value[0]} ${value[1]} ${value[2]})` : fallback;
}

function sameStyle(a: CellStyle, b: CellStyle) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function textTokens(text: string) {
  return text.match(/[\u0600-\u06ff]+|[^\u0600-\u06ff]+/g) ?? [];
}
function trimRightCells<T extends { ch: string }>(cells: T[]) {
  let end = cells.length;
  while (end > 0 && cells[end - 1].ch.trim().length === 0) end -= 1;
  return cells.slice(0, end);
}

export function TerminalCanvas({ frame, theme, onInput }: { frame: TerminalFrame; theme: ThemeName; onInput: (input: string) => void }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;

    const draw = () => {
      const rect = wrap.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.floor(rect.width * dpr);
      canvas.height = Math.floor(rect.height * dpr);
      canvas.style.width = `${rect.width}px`;
      canvas.style.height = `${rect.height}px`;

      const ctx = canvas.getContext('2d');
      if (!ctx) return;
      ctx.scale(dpr, dpr);
      const palette = themes[theme];
      ctx.fillStyle = palette.bg;
      ctx.fillRect(0, 0, rect.width, rect.height);

      const paddingX = 18;
      const paddingY = 16;
      const cellW = Math.max(7, Math.floor((rect.width - paddingX * 2) / frame.cols));
      const cellH = Math.max(16, Math.floor((rect.height - paddingY * 2) / frame.rows));
      const fontPx = Math.max(13, Math.min(17, Math.floor(cellH * 0.72)));
      const fontFamily = '"Cascadia Mono", "JetBrains Mono", "Vazirmatn", "Cairo", monospace';
      ctx.textBaseline = 'middle';
      ctx.textAlign = 'left';

      ctx.strokeStyle = palette.grid;
      ctx.lineWidth = 1;
      for (let row = 0; row <= frame.rows; row += 4) {
        const y = paddingY + row * cellH;
        ctx.beginPath();
        ctx.moveTo(paddingX, y);
        ctx.lineTo(paddingX + frame.cols * cellW, y);
        ctx.stroke();
      }

      for (const line of frame.lines) {
        const y = paddingY + line.row * cellH + cellH / 2;
        const cells = trimRightCells(line.cells);
        if (cells.length === 0) continue;

        for (let i = 0; i < cells.length; i++) {
          const cell = cells[i];
          if (!cell.style.bg) continue;
          ctx.fillStyle = rgb(cell.style.bg);
          ctx.fillRect(paddingX + i * cellW, y - cellH / 2, cellW, cellH);
        }

        let runStart = 0;
        while (runStart < cells.length) {
          const style = cells[runStart].style;
          let runEnd = runStart + 1;
          while (runEnd < cells.length && sameStyle(style, cells[runEnd].style)) runEnd += 1;

          const text = cells.slice(runStart, runEnd).map((cell) => cell.ch).join('');
          if (text.length > 0) {
            ctx.font = `${style.bold ? 700 : 500} ${fontPx}px ${fontFamily}`;
            ctx.fillStyle = style.inverse ? palette.bg : rgb(style.fg);
            let tokenX = paddingX + runStart * cellW + 1;
            for (const token of textTokens(text)) {
              ctx.direction = /[\u0600-\u06ff]/.test(token) ? 'rtl' : 'ltr';
              ctx.fillText(token, tokenX, y);
              tokenX += ctx.measureText(token).width;
            }
            if (style.underline) {
              const width = ctx.measureText(text).width;
              ctx.fillRect(paddingX + runStart * cellW + 1, y + cellH * 0.3, width, 1);
            }
          }
          runStart = runEnd;
        }
      }

      if (frame.cursorVisible) {
        const cursorX = paddingX + frame.cursorCol * cellW;
        const cursorY = paddingY + frame.cursorRow * cellH;
        ctx.fillStyle = themes[theme].cursor;
        ctx.globalAlpha = 0.72;
        ctx.fillRect(cursorX, cursorY + cellH - 4, Math.max(4, cellW - 1), 2);
        ctx.globalAlpha = 1;
      }
    };

    draw();
    const observer = new ResizeObserver(draw);
    observer.observe(wrap);
    return () => observer.disconnect();
  }, [frame, theme]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.ctrlKey || event.metaKey) return;
      if (event.key === 'Enter') onInput('\r');
      else if (event.key === 'Backspace') onInput('\u0008');
      else if (event.key === 'Tab') onInput('\t');
      else if (event.key.length === 1) onInput(event.key);
      else return;
      event.preventDefault();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onInput]);

  return (
    <div ref={wrapRef} className="terminal-canvas" tabIndex={0}>
      <canvas ref={canvasRef} aria-label="RTL terminal output" />
    </div>
  );
}


