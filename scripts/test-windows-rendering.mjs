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
const viteSource = readFileSync(
  new URL('../vite.config.ts', import.meta.url),
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
  tauriConfig.app.security.csp,
  /object-src\s+'none'/,
  'the CSP must block plugin and object embeds',
);
assert.match(
  tauriConfig.app.security.csp,
  /base-uri\s+'none'/,
  'the CSP must block base URL rewriting',
);
assert.match(
  tauriConfig.app.security.csp,
  /form-action\s+'none'/,
  'the CSP must block form submissions',
);
assert.match(
  viteSource,
  /port:\s*1420,[\s\S]*?strictPort:\s*true/,
  'Vite must fail closed when the fixed Tauri development port is occupied',
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

console.log('Windows rendering policy: 11/11 checks passed');
