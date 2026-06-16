import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
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
      allowProposedApi: false,
      fontFamily: "'Cascadia Mono', 'Consolas', 'JetBrains Mono', 'DejaVu Sans Mono', 'Liberation Mono', 'Menlo', 'Noto Naskh Arabic', monospace",
      fontSize: 15,
      lineHeight: 1.12,
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
        term.writeln('English stays LTR · ' + shapeArabicAndBiDi('العربية متصلة وتُعرض من اليمين لليسار') + ' ✓');
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

      cleanupData = await tauri.listen<string>('terminal://data', (event) => {
        const transformed = shapeArabicAndBiDi(event.payload);
        term.write(transformed);
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

// ==========================================
// Robust Arabic Shaping & BiDi Reorder Engine
// ==========================================
type FormMap = {
  isolated: string;
  initial?: string;
  medial?: string;
  final?: string;
  joining: 'dual' | 'right' | 'none';
};

const ARABIC_MAP: Record<string, FormMap> = {
  '\u0621': { isolated: '\uFE80', joining: 'none' }, // Hamza
  '\u0622': { isolated: '\uFE81', final: '\uFE82', joining: 'right' }, // Alef Madda
  '\u0623': { isolated: '\uFE83', final: '\uFE84', joining: 'right' }, // Alef Hamza Above
  '\u0624': { isolated: '\uFE85', final: '\uFE86', joining: 'right' }, // Waw Hamza
  '\u0625': { isolated: '\uFE87', final: '\uFE88', joining: 'right' }, // Alef Hamza Below
  '\u0626': { isolated: '\uFE89', final: '\uFE8A', initial: '\uFE8B', medial: '\uFE8C', joining: 'dual' }, // Yeh Hamza
  '\u0627': { isolated: '\uFE8D', final: '\uFE8E', joining: 'right' }, // Alef
  '\u0628': { isolated: '\uFE8F', final: '\uFE90', initial: '\uFE91', medial: '\uFE92', joining: 'dual' }, // Beh
  '\u0629': { isolated: '\uFE93', final: '\uFE94', joining: 'right' }, // Teh Marbuta
  '\u062A': { isolated: '\uFE95', final: '\uFE96', initial: '\uFE97', medial: '\uFE98', joining: 'dual' }, // Teh
  '\u062B': { isolated: '\uFE99', final: '\uFE9A', initial: '\uFE9B', medial: '\uFE9C', joining: 'dual' }, // Theh
  '\u062C': { isolated: '\uFE9D', final: '\uFE9E', initial: '\uFE9F', medial: '\uFEA0', joining: 'dual' }, // Jeem
  '\u062D': { isolated: '\uFEA1', final: '\uFEA2', initial: '\uFEA3', medial: '\uFEA4', joining: 'dual' }, // Hah
  '\u062E': { isolated: '\uFEA5', final: '\uFEA6', initial: '\uFEA7', medial: '\uFEA8', joining: 'dual' }, // Khah
  '\u062F': { isolated: '\uFEA9', final: '\uFEAA', joining: 'right' }, // Dal
  '\u0630': { isolated: '\uFEAB', final: '\uFEAC', joining: 'right' }, // Thal
  '\u0631': { isolated: '\uFEAD', final: '\uFEAE', joining: 'right' }, // Ra
  '\u0632': { isolated: '\uFEAF', final: '\uFEB0', joining: 'right' }, // Zay
  '\u0633': { isolated: '\uFEB1', final: '\uFEB2', initial: '\uFEB3', medial: '\uFEB4', joining: 'dual' }, // Seen
  '\u0634': { isolated: '\uFEB5', final: '\uFEB6', initial: '\uFEB7', medial: '\uFEB8', joining: 'dual' }, // Sheen
  '\u0635': { isolated: '\uFEB9', final: '\uFEBA', initial: '\uFEBB', medial: '\uFEBC', joining: 'dual' }, // Sad
  '\u0636': { isolated: '\uFEBD', final: '\uFEBE', initial: '\uFEBF', medial: '\uFEC0', joining: 'dual' }, // Dad
  '\u0637': { isolated: '\uFEC1', final: '\uFEC2', initial: '\uFEC3', medial: '\uFEC4', joining: 'dual' }, // Tah
  '\u0638': { isolated: '\uFEC5', final: '\uFEC6', initial: '\uFEC7', medial: '\uFEC8', joining: 'dual' }, // Zah
  '\u0639': { isolated: '\uFEC9', final: '\uFECA', initial: '\uFECB', medial: '\uFECC', joining: 'dual' }, // Ain
  '\u063A': { isolated: '\uFECD', final: '\uFECE', initial: '\uFECF', medial: '\uFED0', joining: 'dual' }, // Ghain
  '\u0641': { isolated: '\uFED1', final: '\uFED2', initial: '\uFED3', medial: '\uFED4', joining: 'dual' }, // Feh
  '\u0642': { isolated: '\uFED5', final: '\uFED6', initial: '\uFED7', medial: '\uFED8', joining: 'dual' }, // Qaf
  '\u0643': { isolated: '\uFED9', final: '\uFEDA', initial: '\uFEDB', medial: '\uFEDC', joining: 'dual' }, // Kaf
  '\u0644': { isolated: '\uFEDD', final: '\uFEDE', initial: '\uFEDF', medial: '\uFEE0', joining: 'dual' }, // Lam
  '\u0645': { isolated: '\uFEE1', final: '\uFEE2', initial: '\uFEE3', medial: '\uFEE4', joining: 'dual' }, // Meem
  '\u0646': { isolated: '\uFEE5', final: '\uFEE6', initial: '\uFEE7', medial: '\uFEE8', joining: 'dual' }, // Noon
  '\u0647': { isolated: '\uFEE9', final: '\uFEEA', initial: '\uFEEB', medial: '\uFEEC', joining: 'dual' }, // Heh
  '\u0648': { isolated: '\uFEED', final: '\uFEEE', joining: 'right' }, // Waw
  '\u0649': { isolated: '\uFEEF', final: '\uFEF0', joining: 'right' }, // Alef Maksura
  '\u064A': { isolated: '\uFEF1', final: '\uFEF2', initial: '\uFEF3', medial: '\uFEF4', joining: 'dual' }, // Yeh

  // Lam-Alef Presentation Forms (Ligatures) mapped to allow proper adjacent joining
  '\uFEFB': { isolated: '\uFEFB', final: '\uFEFC', joining: 'right' },
  '\uFEF5': { isolated: '\uFEF5', final: '\uFEF6', joining: 'right' },
  '\uFEF7': { isolated: '\uFEF7', final: '\uFEF8', joining: 'right' },
  '\uFEF9': { isolated: '\uFEF9', final: '\uFEFA', joining: 'right' },
};

function preprocessLamAlef(word: string): string {
  let processed = '';
  let i = 0;
  while (i < word.length) {
    const ch = word[i];
    const next = word[i + 1];
    if (ch === '\u0644' && next) {
      if (next === '\u0627') { processed += '\uFEFB'; i += 2; continue; }
      if (next === '\u0622') { processed += '\uFEF5'; i += 2; continue; }
      if (next === '\u0623') { processed += '\uFEF7'; i += 2; continue; }
      if (next === '\u0625') { processed += '\uFEF9'; i += 2; continue; }
    }
    processed += ch;
    i++;
  }
  return processed;
}

function shapeArabicWord(word: string): string {
  const chars = [...word];
  const shaped = chars.map((ch, i) => {
    const entry = ARABIC_MAP[ch];
    if (!entry) return ch;

    const prevCh = chars[i - 1];
    const nextCh = chars[i + 1];

    const prevEntry = ARABIC_MAP[prevCh];
    const nextEntry = ARABIC_MAP[nextCh];

    // A letter connects to its PREVIOUS letter only when that previous letter
    // joins forward — i.e. it is dual-joining. Right-joining letters (Alef, Ra,
    // Dal, Waw, …) connect only to what precedes them, never to what follows.
    const joinsPrev = !!prevEntry && prevEntry.joining === 'dual' && entry.joining !== 'none';
    // A letter connects to its NEXT letter only when it is itself dual-joining
    // and the next letter can receive a join (dual or right-joining).
    const joinsNext = entry.joining === 'dual' && !!nextEntry && nextEntry.joining !== 'none';

    if (joinsPrev && joinsNext) {
      return entry.medial || entry.isolated;
    } else if (joinsPrev) {
      return entry.final || entry.isolated;
    } else if (joinsNext) {
      return entry.initial || entry.isolated;
    } else {
      return entry.isolated;
    }
  });

  return shaped.join('');
}

function shapeArabicAndBiDi(text: string): string {
  // Matches any consecutive sequence of standard Arabic characters and presentation forms
  const arabicRegex = /[\u0600-\u06FF\uFE70-\uFEFF\uFEF5-\uFEFC]+/g;

  return text.replace(arabicRegex, (match) => {
    const preprocessed = preprocessLamAlef(match);
    const shaped = shapeArabicWord(preprocessed);
    // Reverse shaped string to present RTL visually in xterm.js LTR layout
    return [...shaped].reverse().join('');
  });
}







