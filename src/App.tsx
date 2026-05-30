import { RotateCcw, Settings, Terminal, Wifi, WifiOff, X } from 'lucide-react';
import { useMemo, useState } from 'react';
import { TerminalCanvas } from './components/TerminalCanvas';
import { useTerminalSession } from './hooks/useTerminalSession';
import type { ThemeName } from './lib/types';

export function App() {
  const [theme, setTheme] = useState<ThemeName>('midnight');
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [fontScale, setFontScale] = useState(100);
  const cols = useMemo(() => (fontScale < 95 ? 112 : fontScale > 110 ? 82 : 96), [fontScale]);
  const rows = useMemo(() => (fontScale < 95 ? 32 : fontScale > 110 ? 24 : 28), [fontScale]);
  const session = useTerminalSession(cols, rows);

  return (
    <main className={`app theme-${theme}`}>
      <div className="chrome" data-tauri-drag-region>
        <div className="tabs">
          <button className="tab active" type="button">
            <Terminal size={16} />
            <span>{session.frame.title}</span>
          </button>
          <button className="tab compact" type="button" aria-label="Close tab">
            <X size={15} />
          </button>
        </div>
        <div className="window-actions">
          <span className={`status ${session.connected ? 'online' : ''}`}>
            {session.connected ? <Wifi size={15} /> : <WifiOff size={15} />}
            {session.status}
          </span>
          <button type="button" className="icon-button" onClick={session.restart} aria-label="Restart terminal" title="Restart terminal">
            <RotateCcw size={17} />
          </button>
          <button type="button" className="icon-button" onClick={() => setSettingsOpen(true)} aria-label="Settings" title="Settings">
            <Settings size={17} />
          </button>
        </div>
      </div>

      <section className="terminal-shell">
        <TerminalCanvas frame={session.frame} theme={theme} onInput={session.write} />
      </section>

      {settingsOpen && (
        <div className="modal-backdrop" onMouseDown={() => setSettingsOpen(false)}>
          <div className="settings-panel" onMouseDown={(event) => event.stopPropagation()}>
            <div className="panel-head">
              <h2>Settings</h2>
              <button className="icon-button" type="button" onClick={() => setSettingsOpen(false)} aria-label="Close settings">
                <X size={17} />
              </button>
            </div>
            <label className="control-row">
              <span>Theme</span>
              <select value={theme} onChange={(event) => setTheme(event.target.value as ThemeName)}>
                <option value="midnight">Midnight cyan</option>
                <option value="emerald">Emerald glass</option>
                <option value="graphite">Graphite amber</option>
              </select>
            </label>
            <label className="control-row">
              <span>Density</span>
              <input min="88" max="118" type="range" value={fontScale} onChange={(event) => setFontScale(Number(event.target.value))} />
            </label>
            <div className="metrics">
              <span>{cols} cols</span>
              <span>{rows} rows</span>
              <span>RTL-first</span>
            </div>
          </div>
        </div>
      )}
    </main>
  );
}
