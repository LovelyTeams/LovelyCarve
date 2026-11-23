import { globFiles } from './lib/files.ts';

// Components and composables reach Tauri's invoke and listen only through the typed IPC layer.
const forbidden = ['@tauri-apps/api/core', '@tauri-apps/api/event'];

const violations: string[] = [];
for (const file of await globFiles('src/**/*.{ts,vue}')) {
  if (file.startsWith('src/lib/ipc/')) continue;
  (await Bun.file(file).text()).split(/\r?\n/).forEach((line, index) => {
    if (forbidden.some(pattern => line.includes(pattern))) violations.push(`${file}:${index + 1}: ${line.trim()}`);
  });
}

if (violations.length > 0) {
  console.error('Direct Tauri core/event imports must go through src/lib/ipc/*.');
  console.error(violations.join('\n'));
  process.exit(1);
}

console.log('IPC boundary OK: no direct core/event imports outside src/lib/ipc.');
