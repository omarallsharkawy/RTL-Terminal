import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';

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

export function XtermTerminal() {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      cursorBlink: true,
      cursorStyle: 'block',
      convertEol: false,
      allowProposedApi: false,
      fontFamily: 'Cascadia Mono, Consolas, Segoe UI, Tahoma, monospace',
      fontSize: 15,
      lineHeight: 1.12,
      letterSpacing: 0,
      scrollback: 10000,
      windowsPty: {
        backend: 'conpty',
        buildNumber: 21376,
      },
      theme: {
        background: '#0c0c0c',
        foreground: '#cccccc',
        cursor: '#cccccc',
        selectionBackground: '#264f78',
        black: '#0c0c0c',
        red: '#f87171',
        green: '#16c60c',
        yellow: '#facc15',
        blue: '#7dd3fc',
        magenta: '#c084fc',
        cyan: '#67e8f9',
        white: '#d8e4ec',
        brightBlack: '#64748b',
        brightRed: '#fca5a5',
        brightGreen: '#bef264',
        brightYellow: '#fde047',
        brightBlue: '#bae6fd',
        brightMagenta: '#d8b4fe',
        brightCyan: '#a5f3fc',
        brightWhite: '#f8fafc',
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();
    term.focus();
    host.addEventListener('mousedown', () => term.focus());
    terminalRef.current = term;
    fitRef.current = fit;

    let cancelled = false;
    let cleanupData: (() => void) | undefined;
    let resizeTimer: number | undefined;

    const resize = async (tauri?: { invoke: Invoke }) => {
      fit.fit();
      const cols = Math.max(20, term.cols);
      const rows = Math.max(6, term.rows);
      if (tauri) await tauri.invoke('resize_terminal', { cols, rows });
    };

    getTauri().then(async (tauri) => {
      if (cancelled) return;
      if (!tauri) {
        term.writeln('RTL Terminal demo');
        term.writeln('npm run tauri:dev لتشغيل التيرمنال الحقيقي');
        term.writeln('English stays LTR, العربية تظهر حسب دعم الخط والمتصفح.');
        return;
      }

      term.attachCustomKeyEventHandler((event) => {
        if (event.type !== 'keydown') return true;

        const plainCtrl = event.ctrlKey && !event.altKey && !event.metaKey;
        if (plainCtrl && event.code === 'KeyC') {
          term.write('\r\n[DEBUG: Frontend captured Ctrl+C]\r\n');
          tauri.invoke('interrupt_terminal').catch((err) => {
            term.write(`\r\n[DEBUG: Tauri invoke error: ${String(err)}]\r\n`);
          });
          return false;
        }
        if (plainCtrl && event.code === 'KeyD') {
          tauri.invoke('write_terminal', { input: String.fromCharCode(4) }).catch(console.error);
          return false;
        }

        return true;
      });

      cleanupData = await tauri.listen<string>('terminal://data', (event) => {
        term.write(event.payload);
      });
      term.onData((data) => {
        tauri.invoke('write_terminal', { input: data }).catch(console.error);
      });
      await tauri.invoke('start_terminal', { cols: term.cols, rows: term.rows });
      await resize(tauri);

      const observer = new ResizeObserver(() => {
        if (resizeTimer) window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => resize(tauri).catch(console.error), 40);
      });
      observer.observe(host);
      window.addEventListener('beforeunload', () => observer.disconnect(), { once: true });
    }).catch((error) => {
      console.error(error);
      term.writeln(`Failed to start PTY: ${String(error)}`);
    });

    return () => {
      cancelled = true;
      cleanupData?.();
      if (resizeTimer) window.clearTimeout(resizeTimer);
      term.dispose();
      terminalRef.current = null;
      fitRef.current = null;
    };
  }, []);

  return <div ref={hostRef} className="xterm-host" />;
}






