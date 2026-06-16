const fs = require('node:fs');
const path = require('node:path');

const roots = [
  'package.json',
  'tsconfig.json',
  'vite.config.ts',
  'src-tauri/Cargo.toml',
  'src-tauri/tauri.conf.json',
  'src-tauri/capabilities/default.json',
  '.github/workflows/build.yml',
];

for (const file of roots) {
  if (!fs.existsSync(file)) continue;
  let text = fs.readFileSync(file, 'utf8');
  if (text.charCodeAt(0) === 0xfeff) {
    text = text.slice(1);
  }
  fs.writeFileSync(file, text, 'utf8');
}

for (const file of ['package.json', 'tsconfig.json', 'src-tauri/tauri.conf.json', 'src-tauri/capabilities/default.json']) {
  JSON.parse(fs.readFileSync(file, 'utf8'));
}

console.log('Config files are UTF-8 without BOM and JSON is valid.');
