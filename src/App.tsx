import { useMemo, useState } from 'react';
import { TerminalCanvas } from './components/TerminalCanvas';
import { useTerminalSession } from './hooks/useTerminalSession';
import type { ThemeName } from './lib/types';

export function App() {
  const [theme] = useState<ThemeName>('midnight');
  const [fontScale] = useState(100);
  const cols = useMemo(() => (fontScale < 95 ? 112 : fontScale > 110 ? 82 : 96), [fontScale]);
  const rows = useMemo(() => (fontScale < 95 ? 34 : fontScale > 110 ? 26 : 30), [fontScale]);
  const session = useTerminalSession(cols, rows);

  return (
    <main className={`app theme-${theme}`}>
      <TerminalCanvas frame={session.frame} theme={theme} onInput={session.write} />
    </main>
  );
}
