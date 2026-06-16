import { useState } from 'react';
import { XtermTerminal } from './components/XtermTerminal';
import { StatusBar, type TerminalStatus } from './components/StatusBar';

export function App() {
  const [status, setStatus] = useState<TerminalStatus>('connecting');
  const [shell, setShell] = useState<string | null>(null);

  return (
    <main className="app">
      <div className="terminal-workspace">
        <XtermTerminal onStatusChange={setStatus} onShellChange={setShell} />
      </div>
      <StatusBar status={status} shell={shell} />
    </main>
  );
}
