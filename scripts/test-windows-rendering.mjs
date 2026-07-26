import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const tauriConfig = JSON.parse(
  readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
);
const terminalSource = readFileSync(
  new URL('../src/components/XtermTerminal.tsx', import.meta.url),
  'utf8',
);
const ptySource = readFileSync(
  new URL('../src-tauri/src/pty.rs', import.meta.url),
  'utf8',
);

assert.deepEqual(
  tauriConfig.bundle.targets,
  ['nsis'],
  'the release bundle must stay scoped to Windows NSIS',
);
assert.ok(
  tauriConfig.app.security.dangerousDisableAssetCspModification.includes('style-src'),
  'Tauri must preserve unsafe-inline for xterm dynamic renderer styles',
);
assert.match(
  tauriConfig.app.security.csp,
  /style-src[^;]*'unsafe-inline'/,
  'the explicit CSP must allow xterm dynamic renderer styles',
);
assert.match(
  ptySource,
  /cmd\.env\("FORCE_COLOR",\s*"3"\)/,
  'Windows PTY children must receive truecolor capability',
);
assert.match(
  terminalSource,
  /brightBlue:\s*'#3b78ff'/,
  'xterm must use the Windows Terminal Campbell bright-blue token',
);
assert.match(
  terminalSource,
  /settleInitialTerminalLayout\([\s\S]*?\(\)\s*=>\s*cancelled/,
  'the PTY must wait for stable maximized geometry before spawning',
);
assert.match(
  terminalSource,
  /if\s*\(!layoutReady\s*\|\|\s*cancelled\)\s*return/,
  'closing the window must cancel initial layout before PTY startup',
);

console.log('Windows rendering policy: 7/7 checks passed');
