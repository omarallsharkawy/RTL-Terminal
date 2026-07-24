import { useState } from 'react';
import { RotateCcw, SquareTerminal } from 'lucide-react';
import { XtermTerminal } from './components/XtermTerminal';
import { StatusBar, type TerminalStatus } from './components/StatusBar';

export function App() {
  const [status, setStatus] = useState<TerminalStatus>('connecting');
  const [shell, setShell] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [retryNonce, setRetryNonce] = useState(0);

  const retry = () => {
    setError(null);
    setShell(null);
    setStatus('connecting');
    setRetryNonce((value) => value + 1);
  };

  return (
    <main className="app">
      <header className="terminal-rail" dir="ltr">
        <div className="terminal-rail__brand" aria-label="Twitty">
          <SquareTerminal size={15} strokeWidth={1.8} aria-hidden="true" />
          <span>Twitty</span>
        </div>
        <div className="terminal-rail__tab" aria-label="Active terminal session">
          <span
            className={`terminal-rail__tab-dot terminal-rail__tab-dot--${status}`}
            aria-hidden="true"
          />
          <span>{shell || 'Terminal'}</span>
        </div>
        <span className="terminal-rail__identity" lang="ar" dir="rtl">
          طرفية عربية
        </span>
      </header>
      <div className="terminal-workspace">
        <XtermTerminal
          onStatusChange={setStatus}
          onShellChange={setShell}
          onErrorChange={setError}
          retryNonce={retryNonce}
        />
        {error && (
          <section className="recovery-notice" role="alert" dir="ltr">
            <div className="recovery-notice__copy">
              <strong>Terminal unavailable</strong>
              <span>{error}</span>
            </div>
            <button className="recovery-notice__action" type="button" onClick={retry}>
              <RotateCcw size={14} strokeWidth={2} aria-hidden="true" />
              Retry
              <span lang="ar" dir="rtl">إعادة المحاولة</span>
            </button>
          </section>
        )}
      </div>
      <StatusBar status={status} shell={shell} />
    </main>
  );
}
