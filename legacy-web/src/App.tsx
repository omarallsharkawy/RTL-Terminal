import { useState } from 'react';
import { RotateCcw } from 'lucide-react';
import { XtermTerminal } from './components/XtermTerminal';

export function App() {
  const [error, setError] = useState<string | null>(null);
  const [retryNonce, setRetryNonce] = useState(0);

  const retry = () => {
    setError(null);
    setRetryNonce((value) => value + 1);
  };

  return (
    <main className="app">
      <div className="terminal-workspace">
        <XtermTerminal
          onErrorChange={setError}
          retryNonce={retryNonce}
        />
        {error && (
          <section className="recovery-notice" role="alert" dir="ltr" lang="en">
            <div className="recovery-notice__copy">
              <strong>Terminal unavailable</strong>
              <span>{error}</span>
            </div>
            <button
              className="recovery-notice__action"
              type="button"
              onClick={retry}
              autoFocus
            >
              <RotateCcw size={14} strokeWidth={2} aria-hidden="true" />
              Retry
              <span lang="ar" dir="rtl">إعادة المحاولة</span>
            </button>
          </section>
        )}
      </div>
    </main>
  );
}
