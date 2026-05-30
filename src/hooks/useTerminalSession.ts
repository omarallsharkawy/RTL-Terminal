import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { TerminalFrame } from '../lib/types';
import { makeMockFrame } from '../lib/mockTerminal';

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, cb: (event: { payload: T }) => void) => Promise<() => void>;

async function getTauri() {
  if (!('__TAURI_INTERNALS__' in window)) return null;
  const [{ invoke }, { listen }] = await Promise.all([
    import('@tauri-apps/api/core'),
    import('@tauri-apps/api/event'),
  ]);
  return { invoke: invoke as Invoke, listen: listen as Listen };
}

export function useTerminalSession(cols: number, rows: number) {
  const [frame, setFrame] = useState<TerminalFrame>(() => makeMockFrame(0, cols, rows));
  const [connected, setConnected] = useState(false);
  const [status, setStatus] = useState('demo');
  const tickRef = useRef(0);
  const tauriRef = useRef<{ invoke: Invoke; listen: Listen } | null>(null);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let timer: number | undefined;

    getTauri().then(async (tauri) => {
      if (cancelled) return;
      tauriRef.current = tauri;
      if (!tauri) {
        setStatus('demo');
        timer = window.setInterval(() => {
          tickRef.current += 1;
          setFrame(makeMockFrame(tickRef.current, cols, rows));
        }, 850);
        return;
      }

      setStatus('starting');
      unlisten = await tauri.listen<TerminalFrame>('terminal://frame', (event) => {
        setFrame(event.payload);
        setConnected(true);
        setStatus('connected');
      });
      await tauri.invoke('start_terminal', { cols, rows });
    }).catch((error) => {
      console.error(error);
      setConnected(false);
      setStatus('error');
    });

    return () => {
      cancelled = true;
      if (timer) window.clearInterval(timer);
      unlisten?.();
    };
  }, [cols, rows]);

  useEffect(() => {
    const tauri = tauriRef.current;
    if (!tauri || status === 'demo') return;
    tauri.invoke('resize_terminal', { cols, rows }).catch(console.error);
  }, [cols, rows, status]);

  const write = useCallback((input: string) => {
    const tauri = tauriRef.current;
    if (!tauri) {
      setFrame((current) => {
        const next = structuredClone(current) as TerminalFrame;
        const row = Math.min(next.cursorRow, next.rows - 1);
        const line = next.lines[row];
        for (const ch of input.replace(/\r/g, '')) {
          if (ch === '\n') continue;
          const col = Math.min(next.cursorCol, next.cols - 1);
          line.cells[col] = { ch, logicalIndex: col, width: 1, style: { fg: [122, 250, 195] } };
          next.cursorCol = Math.min(col + 1, next.cols - 1);
        }
        return next;
      });
      return;
    }
    tauri.invoke('write_terminal', { input }).catch(console.error);
  }, []);

  const restart = useCallback(() => {
    const tauri = tauriRef.current;
    if (!tauri) {
      tickRef.current = 0;
      setFrame(makeMockFrame(0, cols, rows));
      return;
    }
    tauri.invoke('stop_terminal')
      .then(() => tauri.invoke('start_terminal', { cols, rows }))
      .catch(console.error);
  }, [cols, rows]);

  return useMemo(() => ({ frame, connected, status, write, restart }), [frame, connected, status, write, restart]);
}
