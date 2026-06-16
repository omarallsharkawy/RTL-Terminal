import { useEffect, useRef, useState } from 'react';

export type TerminalStatus = 'connecting' | 'connected' | 'reconnecting' | 'demo' | 'error';

const STATE_LABEL: Record<TerminalStatus, string> = {
  connecting: 'Connecting',
  connected: 'Connected',
  reconnecting: 'Reconnecting',
  demo: 'Demo (no PTY)',
  error: 'Disconnected',
};

interface StatusBarProps {
  status: TerminalStatus;
  shell: string | null;
}

/**
 * Thin, non-intrusive terminal status bar. Conveys live connection state, the
 * active shell, and the product's defining capability (bidirectional AR⇄EN).
 * Technical hints stay LTR even though the app shell is RTL.
 */
export function StatusBar({ status, shell }: StatusBarProps) {
  // Brief, accessible "live" announcement when the connection state changes.
  const [announce, setAnnounce] = useState('');
  const first = useRef(true);

  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    setAnnounce(`Terminal ${STATE_LABEL[status].toLowerCase()}`);
  }, [status]);

  const pending = status === 'connecting' || status === 'reconnecting';

  return (
    <footer className="statusbar" dir="ltr">
      <div className="statusbar__group">
        <span
          className={`statusbar__dot statusbar__dot--${status}${pending ? ' is-pulsing' : ''}`}
          aria-hidden="true"
        />
        <span className="statusbar__state">{STATE_LABEL[status]}</span>
        {shell && status === 'connected' && (
          <span className="statusbar__shell" title="Active shell">{shell}</span>
        )}
      </div>

      <div className="statusbar__group statusbar__group--end">
        <span className="statusbar__bidi" title="Bidirectional Arabic + English support">
          AR<span className="statusbar__bidi-arrow">⇄</span>EN
        </span>
        <kbd className="statusbar__key">F11</kbd>
        <span className="statusbar__hint">fullscreen</span>
        <span className="statusbar__sep" aria-hidden="true" />
        <kbd className="statusbar__key">Ctrl+C</kbd>
        <span className="statusbar__hint">interrupt</span>
      </div>

      <span className="statusbar__sr" role="status" aria-live="polite">{announce}</span>
    </footer>
  );
}
