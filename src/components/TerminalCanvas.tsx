import { useEffect, useMemo, useRef, useState } from 'react';
import type { TerminalFrame, ThemeName, CellStyle, VisualCell } from '../lib/types';

const themes: Record<ThemeName, { bg: string; grid: string; cursor: string; selection: string }> = {
  midnight: { bg: '#050708', grid: 'rgba(216, 228, 236, 0)', cursor: '#7dd3fc', selection: 'rgba(125, 211, 252, 0.18)' },
  emerald: { bg: '#06100c', grid: 'rgba(92, 255, 184, 0)', cursor: '#7dd3fc', selection: 'rgba(125, 211, 252, 0.18)' },
  graphite: { bg: '#0b0d0f', grid: 'rgba(239, 246, 255, 0)', cursor: '#f5c542', selection: 'rgba(245, 197, 66, 0.16)' },
};

function rgb(value?: [number, number, number], fallback = '#e4eef7') {
  return value ? `rgb(${value[0]} ${value[1]} ${value[2]})` : fallback;
}

function sameStyle(a: CellStyle, b: CellStyle) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function trimRightCells<T extends { ch: string }>(cells: T[]) {
  let end = cells.length;
  while (end > 0 && cells[end - 1].ch.trim().length === 0) end -= 1;
  return cells.slice(0, end);
}

function lineFlow(text: string): 'rtl' | 'ltr' {
  for (const ch of text) {
    if (/[\u0600-\u06ff]/.test(ch)) return 'rtl';
    if (/[A-Za-z]/.test(ch)) return 'ltr';
  }
  return 'ltr';
}

function fontForText(text: string) {
  return /[\u0600-\u06ff]/.test(text)
    ? '"Segoe UI", "Tahoma", "Arial", sans-serif'
    : '"Cascadia Mono", "Consolas", "JetBrains Mono", monospace';
}

function groupCells(cells: VisualCell[]) {
  const groups: Array<{ text: string; style: CellStyle; key: string }> = [];
  let index = 0;
  while (index < cells.length) {
    const style = cells[index].style;
    let end = index + 1;
    while (end < cells.length && sameStyle(style, cells[end].style)) end += 1;
    groups.push({
      text: cells.slice(index, end).map((cell) => cell.ch).join(''),
      style,
      key: `${index}-${end}`,
    });
    index = end;
  }
  return groups;
}

export function TerminalCanvas({ frame, theme, onInput }: { frame: TerminalFrame; theme: ThemeName; onInput: (input: string) => void }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [metrics, setMetrics] = useState({ paddingX: 18, paddingY: 16, cellW: 8, cellH: 18, fontPx: 14 });

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

      const paddingX = 10;
      const paddingY = 8;
      const cellW = Math.max(7, Math.floor((rect.width - paddingX * 2) / frame.cols));
      const cellH = Math.max(16, Math.floor((rect.height - paddingY * 2) / frame.rows));
      const fontPx = Math.max(13, Math.min(16, Math.floor(cellH * 0.78)));
      setMetrics((current) => {
        if (
          current.paddingX === paddingX &&
          current.paddingY === paddingY &&
          current.cellW === cellW &&
          current.cellH === cellH &&
          current.fontPx === fontPx
        ) {
          return current;
        }
        return { paddingX, paddingY, cellW, cellH, fontPx };
      });

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
        for (let i = 0; i < line.cells.length; i++) {
          const cell = line.cells[i];
          if (!cell.style.bg) continue;
          ctx.fillStyle = rgb(cell.style.bg);
          ctx.fillRect(paddingX + i * cellW, paddingY + line.row * cellH, cellW, cellH);
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

  const renderedLines = useMemo(() => frame.lines.map((line) => {
    const cells = trimRightCells(line.cells);
    const text = cells.map((cell) => cell.ch).join('');
    return { row: line.row, flow: lineFlow(text), groups: groupCells(cells) };
  }), [frame.lines]);

  return (
    <div ref={wrapRef} className="terminal-canvas" tabIndex={0}>
      <canvas ref={canvasRef} aria-label="RTL terminal grid" />
      <div className="terminal-text-layer" aria-label="RTL terminal output">
        {renderedLines.map((line) => (
          <div
            className="terminal-text-line"
            key={line.row}
            dir={line.flow}
            style={{
              top: metrics.paddingY + line.row * metrics.cellH,
              left: metrics.paddingX,
              width: frame.cols * metrics.cellW,
              height: metrics.cellH,
              lineHeight: `${metrics.cellH}px`,
              fontSize: metrics.fontPx,
              textAlign: line.flow === 'rtl' ? 'right' : 'left',
            }}
          >
            {line.groups.map((group) => (
              <span
                key={group.key}
                style={{
                  color: rgb(group.style.fg),
                  fontFamily: fontForText(group.text),
                  fontWeight: group.style.bold ? 700 : 500,
                  fontStyle: group.style.italic ? 'italic' : 'normal',
                  textDecoration: group.style.underline ? 'underline' : 'none',
                  unicodeBidi: 'isolate',
                }}
              >
                {group.text}
              </span>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}



