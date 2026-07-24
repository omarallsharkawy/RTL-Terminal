import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import { createArabicOutputBuffer, shapeArabic } from './arabicReshaper';
import { getReconnectDecision } from './reconnectPolicy';
import type { TerminalStatus } from './StatusBar';

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, cb: (event: { payload: T }) => void) => Promise<() => void>;
type Disposable = { dispose: () => void };

interface XtermTerminalProps {
  onStatusChange?: (status: TerminalStatus) => void;
  onShellChange?: (shell: string | null) => void;
  onErrorChange?: (message: string | null) => void;
  retryNonce?: number;
}

interface StartTerminalResult {
  sessionId: number;
  shell: string;
}

type TerminalDataPayload = string | { sessionId: number; data: string };
type TerminalExitPayload = undefined | null | { sessionId: number };

const ARABIC_OUTPUT_FLUSH_MS = 12;
let nextTerminalSessionId = 0;

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
  const platform = navigator.userAgent.toLowerCase();
  if (platform.includes('windows')) return 'powershell';
  if (platform.includes('mac')) return 'zsh';
  return 'sh';
}

function eventTargetIsInside(host: HTMLElement, target: EventTarget | null) {
  return target instanceof Node && host.contains(target);
}

function describeError(error: unknown) {
  if (error instanceof Error) return error.message;
  if (typeof error === 'object' && error && 'message' in error) {
    return String(error.message);
  }
  return String(error);
}

export function XtermTerminal({
  onStatusChange,
  onShellChange,
  onErrorChange,
  retryNonce = 0,
}: XtermTerminalProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const statusRef = useRef(onStatusChange);
  const shellRef = useRef(onShellChange);
  const errorRef = useRef(onErrorChange);
  statusRef.current = onStatusChange;
  shellRef.current = onShellChange;
  errorRef.current = onErrorChange;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const setStatus = (s: TerminalStatus) => statusRef.current?.(s);
    const setShell = (s: string | null) => shellRef.current?.(s);
    const setError = (message: string | null) => errorRef.current?.(message);
    setStatus('connecting');
    setError(null);

    const term = new Terminal({
      cursorBlink: true,
      cursorStyle: 'block',
      convertEol: false,
      // registerCharacterJoiner is a proposed API in xterm v6; without this it
      // throws on load and the app never mounts.
      allowProposedApi: true,
      fontFamily: "'Cascadia Mono', 'Consolas', 'JetBrains Mono', 'DejaVu Sans Mono', 'Liberation Mono', 'Menlo', 'Noto Naskh Arabic', monospace",
      fontSize: 15,
      // 1.0 so block-element and box-drawing glyphs tile seamlessly between rows.
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

    // xterm's DOM renderer does not always apply Arabic contextual shaping for
    // terminal cells. Incoming Arabic runs are shaped before they reach xterm,
    // while the joiner remains as a harmless renderer fallback.
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
    const focusHandler = () => term.focus();
    host.addEventListener('mousedown', focusHandler);
    terminalRef.current = term;
    fitRef.current = fit;

    let cancelled = false;
    let cleanupData: (() => void) | undefined;
    let cleanupExited: (() => void) | undefined;
    let inputDisposable: Disposable | undefined;
    let resizeTimer: number | undefined;
    let reconnectTimer: number | undefined;
    let pasteHandler: ((e: ClipboardEvent) => void) | undefined;
    let observer: ResizeObserver | undefined;
    let outputFlushTimer: number | undefined;
    let currentSessionId: number | null = null;
    let reconnectAttempts = 0;
    let sessionConnectedAt = 0;
    let tauriForCleanup: { invoke: Invoke } | undefined;
    const outputBuffer = createArabicOutputBuffer({ preserveCellCount: true });

    const flushOutput = () => {
      outputFlushTimer = undefined;
      const output = outputBuffer.flush();
      if (output) term.write(output);
    };

    const writeOutput = (data: string) => {
      const output = outputBuffer.push(data);
      if (output) term.write(output);

      if (outputFlushTimer) window.clearTimeout(outputFlushTimer);
      if (outputBuffer.hasPending) {
        outputFlushTimer = window.setTimeout(flushOutput, ARABIC_OUTPUT_FLUSH_MS);
      }
    };

    const size = () => {
      fit.fit();
      return {
        cols: Math.min(500, Math.max(20, term.cols)),
        rows: Math.min(300, Math.max(6, term.rows)),
      };
    };

    const resize = async (tauri?: { invoke: Invoke }) => {
      const next = size();
      if (tauri && currentSessionId !== null) {
        await tauri.invoke('resize_terminal', { ...next, sessionId: currentSessionId });
      }
    };

    getTauri().then(async (tauri) => {
      if (cancelled) return;
      if (!tauri) {
        setStatus('demo');
        const w = (s = '') => term.writeln(shapeArabic(s));
        const C = (n: number, s: string) => `\x1b[38;5;${n}m${s}\x1b[0m`;
        w(`${C(39, '┌─ ')}\x1b[1m${C(39, 'Twitty')}\x1b[0m ${C(245, '· RTL-first terminal · browser demo (no PTY attached)')} ${C(39, '─┐')}`);
        w('');
        w(C(244, '# Arabic shapes contextually and flows right-to-left, inline with English.'));
        w('English stays LTR · العربية تتشكّل وتُعرض من اليمين لليسار ✓');
        w(`مرحبا بك في ${C(39, 'Twitty')} — طرفية تدعم العربية والإنجليزية معًا`);
        w('');
        w(`${C(35, '❯')} ${C(245, 'git status')}   ${C(245, '# الفرع:')} ${C(39, 'main')} ${C(245, '· نظيف')}`);
        w(`${C(35, '❯')} ${C(245, 'echo')} ${C(215, '"السلام عليكم, world"')}`);
        w('  السلام عليكم, world');
        w('');
        w(C(244, '# Box-drawing + block glyphs render crisply, so TUIs and borders stay intact:'));
        w(`${C(39, '╭────────────────────────────┬──────────╮')}`);
        w(`${C(39, '│')} ${C(252, 'Capability')}                 ${C(39, '│')} ${C(252, 'Status')}   ${C(39, '│')}`);
        w(`${C(39, '├────────────────────────────┼──────────┤')}`);
        w(`${C(39, '│')} Contextual Arabic shaping  ${C(39, '│')} ${C(35, '✓ live')}   ${C(39, '│')}`);
        w(`${C(39, '│')} Per-run BiDi ordering      ${C(39, '│')} ${C(35, '✓ live')}   ${C(39, '│')}`);
        w(`${C(39, '│')} Box-drawing glyphs         ${C(39, '│')} ${C(35, '✓ live')}   ${C(39, '│')}`);
        w(`${C(39, '╰────────────────────────────┴──────────╯')}`);
        w('');
        w(`${C(245, 'Run')} ${C(36, 'npm run tauri:dev')} ${C(245, 'to launch the real terminal.')}`);
        return;
      }

      tauriForCleanup = tauri;

      const invokeForCurrentSession = <T,>(
        command: string,
        args: Record<string, unknown> = {},
      ): Promise<T> | undefined => {
        if (currentSessionId === null) return undefined;
        return tauri.invoke<T>(command, { ...args, sessionId: currentSessionId });
      };

      const startSession = async (reconnecting = false) => {
        if (cancelled) return;
        setStatus(reconnecting ? 'reconnecting' : 'connecting');
        const sessionId = ++nextTerminalSessionId;
        currentSessionId = sessionId;
        const result = await tauri.invoke<StartTerminalResult>('start_terminal', { ...size(), sessionId });
        if (cancelled || currentSessionId !== sessionId) {
          await tauri.invoke('stop_terminal', { sessionId }).catch(console.error);
          return;
        }
        if (result.sessionId !== sessionId) {
          throw new Error(`PTY returned session ${result.sessionId}; expected ${sessionId}`);
        }
        sessionConnectedAt = performance.now();
        setError(null);
        setStatus('connected');
        setShell(result.shell || detectShellName());
      };

      const scheduleReconnect = (error: unknown) => {
        if (cancelled) return;

        const connectedForMs = sessionConnectedAt
          ? performance.now() - sessionConnectedAt
          : 0;
        const decision = getReconnectDecision(reconnectAttempts, connectedForMs);
        sessionConnectedAt = 0;
        currentSessionId = null;
        setShell(null);

        if (!decision) {
          const message = `Unable to start the terminal after several attempts. ${describeError(error)}`;
          setStatus('error');
          setError(message);
          term.writeln(`\r\n\x1b[31mTerminal unavailable:\x1b[0m ${message}`);
          return;
        }

        reconnectAttempts = decision.attempt;
        setStatus('reconnecting');
        if (reconnectTimer) window.clearTimeout(reconnectTimer);
        reconnectTimer = window.setTimeout(() => {
          reconnectTimer = undefined;
          startSession(true).catch(scheduleReconnect);
        }, decision.delayMs);
      };

      term.attachCustomKeyEventHandler((event) => {
        if (event.type !== 'keydown') return true;

        if (event.code === 'F11') {
          toggleFullscreen().catch(console.error);
          return false;
        }

        const plainCtrl = event.ctrlKey && !event.altKey && !event.metaKey;
        if (plainCtrl && event.code === 'KeyC') {
          invokeForCurrentSession('interrupt_terminal')?.catch(console.error);
          return false;
        }
        if (plainCtrl && event.code === 'KeyD') {
          invokeForCurrentSession('write_terminal', {
            input: String.fromCharCode(4),
          })?.catch(console.error);
          return false;
        }

        return true;
      });

      cleanupData = await tauri.listen<TerminalDataPayload>('terminal://data', (event) => {
        const payload = event.payload;
        if (typeof payload === 'string') {
          writeOutput(payload);
          return;
        }
        if (payload.sessionId !== currentSessionId) return;
        writeOutput(payload.data);
      });
      if (cancelled) {
        cleanupData();
        return;
      }

      cleanupExited = await tauri.listen<TerminalExitPayload>('terminal://exited', (event) => {
        const payload = event.payload;
        if (cancelled) return;
        if (payload && payload.sessionId !== currentSessionId) return;
        const exitedSessionId = currentSessionId;
        if (outputFlushTimer) window.clearTimeout(outputFlushTimer);
        flushOutput();
        outputBuffer.reset();
        // A crashed TUI can leave xterm in alternate-screen, mouse-reporting,
        // or other private modes. A new shell must start from a clean terminal
        // state rather than inheriting protocol state from the dead session.
        term.reset();
        if (exitedSessionId !== null) {
          tauri.invoke('stop_terminal', { sessionId: exitedSessionId }).catch(console.error);
        }
        scheduleReconnect(new Error('The shell exited unexpectedly.'));
      });
      if (cancelled) {
        cleanupExited();
        return;
      }

      inputDisposable = term.onData((data) => {
        invokeForCurrentSession('write_terminal', { input: data })?.catch(console.error);
      });

      // Intercept paste only when it targets the terminal. This bypasses platform
      // keyboard-layout paste issues without hijacking paste elsewhere in the app.
      pasteHandler = (e: ClipboardEvent) => {
        const active = document.activeElement;
        const belongsToTerminal = eventTargetIsInside(host, e.target) || eventTargetIsInside(host, active);
        if (!belongsToTerminal) return;

        e.preventDefault();
        e.stopPropagation();
        const text = e.clipboardData?.getData('text');
        if (text) {
          invokeForCurrentSession('write_terminal', { input: text })?.catch(console.error);
        }
      };
      window.addEventListener('paste', pasteHandler, true);

      observer = new ResizeObserver(() => {
        if (resizeTimer) window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => resize(tauri).catch(console.error), 40);
      });
      observer.observe(host);

      try {
        await startSession();
        if (!cancelled) await resize(tauri);
      } catch (error) {
        scheduleReconnect(error);
      }
    }).catch((error) => {
      console.error(error);
      if (!cancelled) {
        setStatus('error');
        const message = `Failed to initialize the terminal bridge. ${describeError(error)}`;
        setError(message);
        term.writeln(`\x1b[31mTerminal unavailable:\x1b[0m ${message}`);
      }
    });

    return () => {
      cancelled = true;
      const sessionToStop = currentSessionId;
      currentSessionId = null;
      cleanupData?.();
      cleanupExited?.();
      inputDisposable?.dispose();
      observer?.disconnect();
      if (resizeTimer) window.clearTimeout(resizeTimer);
      if (reconnectTimer) window.clearTimeout(reconnectTimer);
      if (outputFlushTimer) window.clearTimeout(outputFlushTimer);
      outputBuffer.reset();
      if (pasteHandler) window.removeEventListener('paste', pasteHandler, true);
      host.removeEventListener('mousedown', focusHandler);
      if (sessionToStop !== null) {
        tauriForCleanup?.invoke('stop_terminal', { sessionId: sessionToStop }).catch(console.error);
      }
      term.dispose();
      terminalRef.current = null;
      fitRef.current = null;
    };
  }, [retryNonce]);

  return <div ref={hostRef} className="xterm-host" dir="ltr" />;
}
