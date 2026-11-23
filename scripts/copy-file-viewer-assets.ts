// 一次性填充 public/file-viewer（Flyfish File Viewer 运行时资产）。
//
// 背景:file-viewer vite-plugin 的 copyAssets 默认每次 dev 启动都在
// configureServer 里全量重写 ~2881 个文件(~160MB),对 watcher 产生数千条
// 事件,实测把 vite 启动从 ~5s 拖到 ~170s。因此 vite.config.ts 把拷贝改为
// `mode: 'build'`(生产构建照常拷贝到 dist),dev 不再拷贝;本脚本在 `just dev`
// / `just dev-fe` 前执行,按 file-viewer-copy-assets 包版本判断:
//   - public/file-viewer 不存在或版本戳不同 → 全量重拷并写版本戳
//   - 版本一致 → 立即退出(毫秒级)
// 升级 @file-viewer/* 依赖后版本戳自动失配,下次 dev 会重新同步。
import { cp, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';

const projectRoot = join(import.meta.dir, '..');
const targetRoot = join(projectRoot, 'public', 'file-viewer');
const stamp = Bun.file(join(targetRoot, '.lc-assets-version'));

const packageJsonPath = Bun.resolveSync('file-viewer-copy-assets/package.json', projectRoot);
const sourceRoot = join(dirname(packageJsonPath), 'viewer');
const { version } = await Bun.file(packageJsonPath).json() as { version: string };

if (!await Bun.file(join(sourceRoot, 'flyfish-viewer-manifest.json')).exists()) {
  console.error(`[file-viewer-assets] 未找到资产源 ${sourceRoot},请先 bun install`);
  process.exit(1);
}

// 版本未变,无需拷贝;戳缺失或损坏时重拷
if ((await stamp.text().catch(() => null))?.trim() === version) process.exit(0);

const started = performance.now();
await rm(targetRoot, { recursive: true, force: true });
await cp(sourceRoot, targetRoot, { recursive: true });
await Bun.write(stamp, version);
console.log(`[file-viewer-assets] 已同步 file-viewer-copy-assets@${version} → public/file-viewer (${Math.round(performance.now() - started)}ms)`);
