import { useEffect, useRef } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import '@xterm/xterm/css/xterm.css';
import {
  findArabicJoinRanges,
  findArabicRenderGroups,
  includeAdjacentCursorInRenderGroups,
  isArabicOnlyRenderRun,
  isNeutralRenderRun,
  logicalArabicRunForVisualText,
  shouldRenderLineRtl,
} from './arabicRenderer';
import { getReconnectDecision } from './reconnectPolicy';
import { updatePendingTerminalInput } from './terminalInputMirror';
import { TerminalInputQueue } from './terminalInputQueue';

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, cb: (event: { payload: T }) => void) => Promise<() => void>;
type Disposable = { dispose: () => void };
type TerminalStatus = 'connecting' | 'connected' | 'reconnecting' | 'demo' | 'error';

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

let nextTerminalSessionId = 0;

async function getTauri() {
  if (!('__TAURI_INTERNALS__' in window)) return null;
  return { invoke: tauriInvoke as Invoke, listen: tauriListen as Listen };
}

async function toggleFullscreen() {
  if (!('__TAURI_INTERNALS__' in window)) return;
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

function nextFrame() {
  return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
}

async function settleInitialTerminalLayout(
  term: Terminal,
  fit: FitAddon,
  host: HTMLElement,
) {
  // WebView2 can report the configured 1180px window for the first frame and
  // maximize it immediately afterwards. Starting the PTY during that window
  // produces an 80/132-column TUI header that never redraws at full width.
  await document.fonts?.ready;

  let previousSignature = '';
  let stableFrames = 0;
  for (let frame = 0; frame < 12; frame += 1) {
    await nextFrame();
    fit.fit();
    const signature = `${host.clientWidth}x${host.clientHeight}:${term.cols}x${term.rows}`;
    stableFrames = signature === previousSignature ? stableFrames + 1 : 0;
    previousSignature = signature;
    if (stableFrames >= 2) return;
  }
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
      fontFamily: "'Cascadia Mono', 'Cascadia Code', 'Consolas', 'Segoe UI', 'Noto Naskh Arabic', monospace",
      fontSize: 15,
      fontWeight: 400,
      fontWeightBold: 600,
      // 1.0 so block-element and box-drawing glyphs tile seamlessly between rows.
      lineHeight: 1.0,
      letterSpacing: 0,
      scrollback: 10000,
      windowsPty: {
        backend: 'conpty',
        buildNumber: 21376,
      },
      theme: {
        // Windows Terminal's Campbell palette. Matching the system terminal
        // keeps ANSI applications visually consistent instead of recoloring
        // their semantic output with a custom pastel theme.
        background: '#0c0c0c',
        foreground: '#cccccc',
        cursor: '#ffffff',
        selectionBackground: '#ffffff40',
        black: '#0c0c0c',
        red: '#c50f1f',
        green: '#13a10e',
        yellow: '#c19c00',
        blue: '#0037da',
        magenta: '#881798',
        cyan: '#3a96dd',
        white: '#cccccc',
        brightBlack: '#767676',
        brightRed: '#e74856',
        brightGreen: '#16c60c',
        brightYellow: '#f9f1a5',
        brightBlue: '#3b78ff',
        brightMagenta: '#b4009e',
        brightCyan: '#61d6d6',
        brightWhite: '#f2f2f2',
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);

    // Keep the PTY stream byte-for-byte correct. xterm's DOM character joiner
    // renders each complete Arabic phrase as one browser text run, so shaping
    // is recalculated from the full visible line after every typed character.
    // This avoids corrupting shell input with presentation forms and avoids
    // timing-dependent shaping when a shell echoes one character at a time.
    term.registerCharacterJoiner(findArabicJoinRanges);

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
    let arabicLayoutObserver: MutationObserver | undefined;
    let currentSessionId: number | null = null;
    let reconnectAttempts = 0;
    let sessionConnectedAt = 0;
    let tauriForCleanup: { invoke: Invoke } | undefined;
    let inputQueue: TerminalInputQueue | undefined;
    let pendingUserInput = '';
    const tuiVisualTextState = new WeakMap<HTMLElement, { visual: string; logical: string }>();

    const unwrapArabicGroups = (row: HTMLElement) => {
      for (const group of Array.from(row.querySelectorAll('.xterm-arabic-group'))) {
        group.replaceWith(...Array.from(group.childNodes));
      }
    };

    const rendererSpans = (row: HTMLElement) => Array.from(row.children).flatMap(
      (child) => {
        if (!(child instanceof HTMLElement) || child.tagName !== 'SPAN') return [];
        if (!child.classList.contains('xterm-arabic-group')) return [child];
        return Array.from(child.children).filter(
          (nested): nested is HTMLElement => (
            nested instanceof HTMLElement && nested.tagName === 'SPAN'
          ),
        );
      },
    );

    const setArabicSpanLayout = (span: HTMLElement, enabled: boolean) => {
      if (!enabled) {
        span.classList.remove('xterm-arabic-run');
        span.style.removeProperty('--terminal-arabic-run-width');
        span.removeAttribute('dir');
        return;
      }

      // xterm allocated this exact width from terminal cells. Preserve it
      // while Windows shapes the Arabic text naturally inside those cells.
      const allocatedWidth = span.getBoundingClientRect().width;
      span.style.setProperty('--terminal-arabic-run-width', `${allocatedWidth}px`);
      span.classList.add('xterm-arabic-run');
      span.setAttribute('dir', 'rtl');
    };

    const restoreTuiLogicalArabicText = (
      span: HTMLElement,
      enabled: boolean,
    ): boolean => {
      const current = span.textContent || '';
      const previous = tuiVisualTextState.get(span);
      if (enabled && previous?.logical === current) return true;
      if (!enabled) {
        tuiVisualTextState.delete(span);
        return false;
      }

      const logical = logicalArabicRunForVisualText(current, pendingUserInput);
      if (!logical) {
        tuiVisualTextState.delete(span);
        return false;
      }

      tuiVisualTextState.set(span, { visual: current, logical });
      span.textContent = logical;
      return true;
    };

    const decorateArabicRow = (row: HTMLElement) => {
      const hasCursor = Boolean(row.querySelector('.xterm-cursor'));

      const findRowArabicGroups = (spans: HTMLElement[]) => {
        const groups = findArabicRenderGroups(
          spans,
          (span) => isArabicOnlyRenderRun(span.textContent || ''),
          (span) => isNeutralRenderRun(span.textContent || ''),
        ).filter((group) => group.length >= 2);

        return hasCursor
          ? includeAdjacentCursorInRenderGroups(
            groups,
            spans,
            (span) => span.classList.contains('xterm-cursor'),
            (span) => isNeutralRenderRun(span.textContent || ''),
          )
          : groups;
      };

      let childSpans = rendererSpans(row);
      const hasPaintedTuiCursor = childSpans.some((span) => (
        /^\s*$/u.test(span.textContent || '')
        && Array.from(span.classList).some((name) => /^xterm-bg-\d+$/u.test(name))
      ));
      const looksLikeTuiPrompt = /^\s*[>❯›»][\s\u00a0]/u.test(row.textContent || '');
      const mayContainVisualTuiInput = hasPaintedTuiCursor || looksLikeTuiPrompt;
      let restoredTuiInput = false;
      for (const span of childSpans) {
        restoredTuiInput = restoreTuiLogicalArabicText(
          span,
          mayContainVisualTuiInput,
        ) || restoredTuiInput;
      }
      row.classList.toggle('xterm-tui-logical-input', restoredTuiInput);

      for (const span of childSpans) {
        setArabicSpanLayout(span, isArabicOnlyRenderRun(span.textContent || ''));
      }

      row.classList.toggle(
        'xterm-rtl-line',
        shouldRenderLineRtl(row.textContent || '', hasCursor),
      );

      // Keep prompts and cursor rows on xterm's LTR grid, but still group every
      // adjacent Arabic renderer span. TUIs commonly split one input phrase by
      // cursor state, syntax color, or repaint boundaries; without this local
      // wrapper its words appear in logical LTR span order. The inline wrapper
      // retains the exact allocated cell width and does not move TUI geometry.
      let groups = findRowArabicGroups(childSpans);

      const existingWrappers = Array.from(
        row.querySelectorAll<HTMLElement>(':scope > .xterm-arabic-group'),
      );
      const wrappersAreCanonical = (
        existingWrappers.length === groups.length
        && groups.every((group) => {
          const parent = group[0].parentElement;
          return (
            parent?.classList.contains('xterm-arabic-group')
            && group.every((span) => span.parentElement === parent)
            && parent.children.length === group.length
          );
        })
      );

      // Our own observer callback must become a true no-op after one pass.
      // Rebuilding a correct wrapper here would create an endless async
      // MutationObserver unwrap/wrap cycle.
      if (wrappersAreCanonical) return;

      if (existingWrappers.length) {
        unwrapArabicGroups(row);
        childSpans = rendererSpans(row);
        groups = findRowArabicGroups(childSpans);
      }

      for (const group of groups) {
        const wrapper = document.createElement('span');
        wrapper.className = 'xterm-arabic-group';
        wrapper.setAttribute('dir', 'rtl');
        group[0].before(wrapper);
        wrapper.append(...group);
      }
    };

    const rowContainer = host.querySelector<HTMLElement>('.xterm-rows');
    if (rowContainer) {
      arabicLayoutObserver = new MutationObserver((mutations) => {
        const changedRows = new Set<HTMLElement>();
        for (const mutation of mutations) {
          const target = mutation.target instanceof HTMLElement
            ? mutation.target
            : mutation.target.parentElement;
          const row = target?.matches('.xterm-rows > div')
            ? target
            : target?.closest<HTMLElement>('.xterm-rows > div');
          if (row) changedRows.add(row);
        }
        for (const row of changedRows) decorateArabicRow(row);
      });
      arabicLayoutObserver.observe(rowContainer, {
        childList: true,
        characterData: true,
        subtree: true,
      });
      for (const row of rowContainer.children) {
        if (row instanceof HTMLElement) decorateArabicRow(row);
      }
    }

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
        const w = (s = '') => term.writeln(s);
        const C = (n: number, s: string) => `\x1b[38;5;${n}m${s}\x1b[0m`;
        w(`${C(39, '┌─ ')}\x1b[1m${C(39, 'Twitty')}\x1b[0m ${C(245, '· RTL-first terminal · browser demo (no PTY attached)')} ${C(39, '─┐')}`);
        w('');
        w(C(244, '# Arabic shapes contextually and flows right-to-left, inline with English.'));
        w('English stays LTR · العربية تتشكّل وتُعرض من اليمين لليسار ✓');
        w(`مرحبا بك في ${C(39, 'Twitty')} — طرفية تدعم العربية والإنجليزية معًا`);
        w(
          `${C(12, 'agy')} هو الواجهة السطرية ${C(244, '(CLI)')} لمنصة `
          + `${C(15, 'Google Antigravity')} للذكاء الاصطناعي والمساعدة البرمجية `
          + `${C(244, '(Terminal).')}`,
        );
        w(`${C(15, 'أهلاً وسهلاً!')} ${C(15, "I'm doing great, thank you for asking!")} 😊`);
        w(`ANSI split: ${C(196, 'مرحبا')}${C(244, ' ')}${C(46, 'بالعالم')} | English123 🌞`);
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
        // Model an active TUI prompt whose renderer splits one Arabic phrase
        // across colored spans, as Claude Code, Kimi Code, and similar CLIs do.
        term.write(`${C(35, '❯')} ${C(15, 'مرحبا')}${C(245, ' ')}${C(39, 'بالعالم')}`);
        return;
      }

      tauriForCleanup = tauri;
      inputQueue = new TerminalInputQueue(
        async (sessionId, input) => {
          if (cancelled || currentSessionId !== sessionId) return;
          await tauri.invoke('write_terminal', { input, sessionId });
        },
        console.error,
      );

      const startSession = async (reconnecting = false) => {
        if (cancelled) return;
        setStatus(reconnecting ? 'reconnecting' : 'connecting');
        const sessionId = ++nextTerminalSessionId;
        currentSessionId = sessionId;
        pendingUserInput = '';
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
        pendingUserInput = '';
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
          pendingUserInput = '';
          if (currentSessionId !== null) {
            inputQueue?.enqueueOperation(
              currentSessionId,
              (sessionId) => tauri.invoke('interrupt_terminal', { sessionId }),
            );
          }
          return false;
        }
        if (plainCtrl && event.code === 'KeyD') {
          if (currentSessionId !== null) {
            inputQueue?.enqueue(currentSessionId, String.fromCharCode(4));
          }
          return false;
        }

        return true;
      });

      [cleanupData, cleanupExited] = await Promise.all([
        tauri.listen<TerminalDataPayload>('terminal://data', (event) => {
          const payload = event.payload;
          if (typeof payload === 'string') {
            term.write(payload);
            return;
          }
          if (payload.sessionId !== currentSessionId) return;
          term.write(payload.data);
        }),
        tauri.listen<TerminalExitPayload>('terminal://exited', (event) => {
          const payload = event.payload;
          if (cancelled) return;
          if (payload && payload.sessionId !== currentSessionId) return;
          const exitedSessionId = currentSessionId;
          // A crashed TUI can leave xterm in alternate-screen, mouse-reporting,
          // or other private modes. A new shell must start from a clean terminal
          // state rather than inheriting protocol state from the dead session.
          term.reset();
          pendingUserInput = '';
          if (exitedSessionId !== null) {
            tauri.invoke('stop_terminal', { sessionId: exitedSessionId }).catch(console.error);
          }
          scheduleReconnect(new Error('The shell exited unexpectedly.'));
        }),
      ]);
      if (cancelled) {
        cleanupData();
        cleanupExited();
        return;
      }

      inputDisposable = term.onData((data) => {
        if (currentSessionId !== null) {
          pendingUserInput = updatePendingTerminalInput(pendingUserInput, data);
          inputQueue?.enqueue(currentSessionId, data);
        }
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
        if (text && currentSessionId !== null) {
          pendingUserInput = updatePendingTerminalInput(pendingUserInput, text);
          inputQueue?.enqueue(currentSessionId, text);
        }
      };
      window.addEventListener('paste', pasteHandler, true);

      observer = new ResizeObserver(() => {
        if (resizeTimer) window.clearTimeout(resizeTimer);
        resizeTimer = window.setTimeout(() => resize(tauri).catch(console.error), 40);
      });
      observer.observe(host);

      try {
        await settleInitialTerminalLayout(term, fit, host);
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
      arabicLayoutObserver?.disconnect();
      if (resizeTimer) window.clearTimeout(resizeTimer);
      if (reconnectTimer) window.clearTimeout(reconnectTimer);
      inputQueue?.dispose();
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
