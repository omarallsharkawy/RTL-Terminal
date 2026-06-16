import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import type { TerminalStatus } from './StatusBar';

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, cb: (event: { payload: T }) => void) => Promise<() => void>;

interface XtermTerminalProps {
  onStatusChange?: (status: TerminalStatus) => void;
  onShellChange?: (shell: string | null) => void;
}

async function getTauri() {
  if (!('__TAURI_INTERNALS__' in window)) return null;
  const [{ invoke }, { listen }] = await Promise.all([
    import('@tauri-apps/api/core'),
    import('@tauri-apps/api/event'),
  ]);
  return { invoke: invoke as Invoke, listen: listen as Listen };
}

async function toggleFullscreen() {
  if (!('__TAURI_INTERNALS__' in window)) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  const win = getCurrentWindow();
  const isFull = await win.isFullscreen();
  await win.setFullscreen(!isFull);
}

function detectShellName(): string {
  // Mirrors the backend's default_shell() so the status bar reflects what spawned.
  const platform = navigator.userAgent.toLowerCase();
  if (platform.includes('windows')) return 'powershell';
  if (platform.includes('mac')) return 'zsh';
  return 'bash';
}

export function XtermTerminal({ onStatusChange, onShellChange }: XtermTerminalProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const statusRef = useRef(onStatusChange);
  const shellRef = useRef(onShellChange);
  statusRef.current = onStatusChange;
  shellRef.current = onShellChange;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const setStatus = (s: TerminalStatus) => statusRef.current?.(s);
    const setShell = (s: string | null) => shellRef.current?.(s);
    setStatus('connecting');

    const term = new Terminal({
      cursorBlink: true,
      cursorStyle: 'block',
      convertEol: false,
      // registerCharacterJoiner (the Arabic RTL render hook) is a proposed API in
      // xterm v6; without this it throws on load and the app never mounts.
      allowProposedApi: true,
      fontFamily: "'Cascadia Mono', 'Consolas', 'JetBrains Mono', 'DejaVu Sans Mono', 'Liberation Mono', 'Menlo', 'Noto Naskh Arabic', monospace",
      fontSize: 15,
      // 1.0 so block-element and box-drawing glyphs tile seamlessly between rows
      // (anything taller inserts vertical gaps that shatter TUI logos and borders).
      lineHeight: 1.0,
      letterSpacing: 0,
      scrollback: 10000,
      windowsPty: {
        backend: 'conpty',
        buildNumber: 21376,
      },
      theme: {
        background: '#0b0d10',
        foreground: '#ccd2d7',
        cursor: '#b4bfca',
        selectionBackground: '#295b77',
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

    // GPU renderer: draws box-drawing + block-element glyphs as crisp, cell-filling
    // custom glyphs so TUI logos and borders render correctly (the DOM renderer pulls
    // them from the font and leaves gaps). Falls back to the DOM renderer if the
    // webview can't provide a WebGL context or loses it.
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        webgl.dispose();
      });
      term.loadAddon(webgl);
    } catch (err) {
      console.warn('WebGL renderer unavailable, using DOM renderer.', err);
    }

    // Arabic shaping + RTL done at the RENDER layer, not on the byte stream. The
    // WebGL renderer draws each joined cell range through canvas fillText, which
    // applies the browser's native Arabic contextual shaping and right-to-left
    // ordering *within the run*. The buffer stays in logical order, so cursor
    // positioning is never disturbed and TUI redraws don't corrupt. A run includes
    // spaces between Arabic words (so multi-word phrases order RTL) but must begin
    // and end on an Arabic letter. Joiners are consumed only by the WebGL renderer.
    const A = '\u0600-\u06FF\u0750-\u077F\u08A0-\u08FF\uFB50-\uFDFF\uFE70-\uFEFF';
    const arabicRun = new RegExp(`[${A}](?:[${A} ]*[${A}])?`, 'g');
    term.registerCharacterJoiner((line) => {
      const ranges: [number, number][] = [];
      arabicRun.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = arabicRun.exec(line)) !== null) {
        if (m[0].length > 1) ranges.push([m.index, m.index + m[0].length]);
      }
      return ranges;
    });

    fit.fit();
    term.focus();
    host.addEventListener('mousedown', () => term.focus());
    terminalRef.current = term;
    fitRef.current = fit;

    let cancelled = false;
    let cleanupData: (() => void) | undefined;
    let resizeTimer: number | undefined;
    let pasteHandler: ((e: ClipboardEvent) => void) | undefined;

    const resize = async (tauri?: { invoke: Invoke }) => {
      fit.fit();
      const cols = Math.max(20, term.cols);
      const rows = Math.max(6, term.rows);
      if (tauri) await tauri.invoke('resize_terminal', { cols, rows });
    };

    getTauri().then(async (tauri) => {
      if (cancelled) return;
      if (!tauri) {
        setStatus('demo');
        term.writeln('\x1b[1mTwitty\x1b[0m \x1b[2m·\x1b[0m browser demo (no PTY attached)');
        term.writeln('');
        term.writeln('\x1b[2mRun\x1b[0m \x1b[36mnpm run tauri:dev\x1b[0m \x1b[2mto launch the real terminal.\x1b[0m');
        term.writeln('English stays LTR · العربية تتشكّل وتُعرض من اليمين لليسار ✓');
        return;
      }

      term.attachCustomKeyEventHandler((event) => {
        if (event.type !== 'keydown') return true;

        if (event.code === 'F11') {
          toggleFullscreen().catch(console.error);
          return false;
        }

        const plainCtrl = event.ctrlKey && !event.altKey && !event.metaKey;
        if (plainCtrl && event.code === 'KeyC') {
          tauri.invoke('interrupt_terminal').catch(console.error);
          return false;
        }
        if (plainCtrl && event.code === 'KeyD') {
          tauri.invoke('write_terminal', { input: String.fromCharCode(4) }).catch(console.error);
          return false;
        }

        return true;
      });

      // Write raw PTY bytes unchanged. Arabic shaping + RTL is handled entirely at
      // the render layer by the character joiner above, so the buffer stays in
      // logical order — correct in both the normal shell and TUI alt-screen apps,
      // with no cursor desync or duplicated-redraw corruption.
      cleanupData = await tauri.listen<string>('terminal://data', (event) => {
        term.write(event.payload);
      });

      const cleanupExited = await tauri.listen('terminal://exited', async () => {
        setStatus('reconnecting');
        await tauri.invoke('start_terminal', { cols: term.cols, rows: term.rows });
        setStatus('connected');
      });

      term.onData((data) => {
        tauri.invoke('write_terminal', { input: data }).catch(console.error);
      });

      // Intercept the browser's default paste event in the capturing phase at the window level.
      // This guarantees we intercept the paste BEFORE xterm.js's hidden textarea consumes it,
      // and completely bypasses the OS-level keyboard layout translation bug on Linux (X11/Wayland).
      pasteHandler = (e: ClipboardEvent) => {
        e.preventDefault();
        e.stopPropagation();
        const text = e.clipboardData?.getData('text');
        if (text) {
          tauri.invoke('write_terminal', { input: text }).catch(console.error);
        }
      };
      window.addEventListener('paste', pasteHandler, true);

      await tauri.invoke('start_terminal', { cols: term.cols, rows: term.rows });
      await resize(tauri);
      setStatus('connected');
      setShell(detectShellName());

      const observer = new ResizeObserver(() => {
        if (resizeTimer) window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => resize(tauri).catch(console.error), 40);
      });
      observer.observe(host);
      window.addEventListener('beforeunload', () => observer.disconnect(), { once: true });
    }).catch((error) => {
      console.error(error);
      setStatus('error');
      term.writeln(`\x1b[31mFailed to start PTY:\x1b[0m ${String(error)}`);
    });

    return () => {
      cancelled = true;
      cleanupData?.();
      if (resizeTimer) window.clearTimeout(resizeTimer);
      if (pasteHandler) window.removeEventListener('paste', pasteHandler, true);
      term.dispose();
      terminalRef.current = null;
      fitRef.current = null;
    };
  }, []);

  return <div ref={hostRef} className="xterm-host" />;
}
