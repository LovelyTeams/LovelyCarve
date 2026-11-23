import { defineConfig, type Plugin } from 'vite';
import vue from '@vitejs/plugin-vue';
import { fileViewerRenderers } from '@file-viewer/vite-plugin';
import Module from 'node:module';
import { isAbsolute, relative, sep } from 'node:path';
import { fileURLToPath, URL } from 'node:url';
import pkg from './package.json' with { type: 'json' };

type ResolveFilename = (
  this: unknown,
  request: string,
  parent: { filename?: string | null } | undefined,
  isMain: boolean,
  options?: { paths?: string[] },
) => string;

// @file-viewer/vite-plugin 3.1.2 的 config() 生成 alias / include 时,每查一个包都把
// preset-all 的几十个锚点包重新 require.resolve 一遍且不缓存:实测 27.9 万次解析里
// 只有 1.8 千次不同,每次启动 vite(dev / build / vitest)都要多花时间:旧运行时上约 25s,
// Bun 1.4.5 上约 0.5s。该钩子是同步的,
// 执行期间 node_modules 不会变,所以只在钩子内缓存 Node 的解析结果(含失败),
// 插件产出的配置不变。上游修复后可删除。
function withMemoizedConfigResolution(plugin: Plugin): Plugin {
  const { config } = plugin;
  const loader = Module as unknown as { _resolveFilename?: ResolveFilename };
  if (typeof config !== 'function' || typeof loader._resolveFilename !== 'function') return plugin;
  return {
    ...plugin,
    config(...args) {
      const original = loader._resolveFilename!;
      const cache = new Map<string, { value: string } | { error: unknown }>();
      loader._resolveFilename = function (request, parent, isMain, options) {
        const key = `${request}\0${parent?.filename ?? ''}\0${options?.paths?.join('\0') ?? ''}`;
        let entry = cache.get(key);
        if (!entry) {
          try {
            entry = { value: original.call(this, request, parent, isMain, options) };
          } catch (error) {
            entry = { error };
          }
          cache.set(key, entry);
        }
        if ('error' in entry) throw entry.error;
        return entry.value;
      };
      try {
        return config.apply(this, args);
      } finally {
        loader._resolveFilename = original;
      }
    },
  };
}

// 前端只用到 src/、public/ 和仓库根下的单个文件(index.html、各配置);其余顶层目录
// (src-tauri/target、plugins/target、opencv 源码、fixtures、.claude/worktrees …)
// 合计约 44 万个文件。根外路径(配置依赖等)照常监听。
const projectRoot = fileURLToPath(new URL('.', import.meta.url));
const frontendDirs = new Set(['src', 'public']);
function isOutsideFrontend(path: string): boolean {
  const relativePath = relative(projectRoot, path);
  if (relativePath.startsWith('..') || isAbsolute(relativePath)) return false;
  const [topLevel, ...rest] = relativePath.split(sep);
  return rest.length > 0 && !frontendDirs.has(topLevel);
}

// Tauri expects a fixed port; fail if the port is already in use.
export default defineConfig({
  plugins: [
    vue(),
    // Flyfish File Viewer (RichPreviewViewer): publish the worker / WASM / vendor
    // assets the library fetches from `/file-viewer/...` at runtime into dist/
    // (build only — see copyAssets below). `inject: false` keeps the plugin from
    // injecting the virtual renderer module into index.html — the full preset
    // is already baked into `@file-viewer/vue3-full` and its renderers lazy-
    // load per extension; injecting would pull them all into the eager bundle.
    withMemoizedConfigResolution(fileViewerRenderers({
      // 只在生产构建时拷贝资产(closeBundle → dist)。dev 不再拷贝:插件默认
      // 每次启动都在 configureServer 里全量重写 ~2881 个文件,watcher 对每次
      // 写入发事件,实测把 vite 启动从 ~5s 拖到 ~170s(见 git blame 同提交)。
      // dev 下的 public/file-viewer 由 scripts/copy-file-viewer-assets.ts
      // 按版本一次性填充,`just dev` / `just dev-fe` 已内置。
      copyAssets: { mode: 'build' },
      inject: false,
      preset: 'all',
      missingRenderer: 'warn',
    })),
  ],
  clearScreen: false,
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
    __APP_BUILD_DATE__: JSON.stringify(new Date().toISOString()),
  },
  server: {
    port: 14200,
    strictPort: true,
    host: '127.0.0.1',
    watch: {
      // vite 8 内置 chokidar 3,在 Windows 上给每个文件、目录各开一个 fs.watch 句柄。
      // 监听整个仓库根实测爬了 7 分钟以上、44 万句柄、4 GB 内存,期间 dev server
      // 几乎无响应,tauri 窗口一直白屏。
      ignored: isOutsideFrontend,
    },
  },
  optimizeDeps: {
    // 不设 entries 时扫描器把仓库里所有 **/*.html 都当入口(只排除 node_modules 与
    // dist):要先遍历约 44 万个文件,再爬 opencv 文档、cargo target 产物、其他
    // worktree 等 ~150 个无关页面。应用只有这一个入口(预览窗口同页 ?view=)。
    entries: ['index.html'],
    // vite 8 的依赖扫描(rolldown 原生,无 JS 日志)跟随动态 import 爬全图,
    // @file-viewer/vue3-full → preset-all → 40 个渲染器(pdfjs/three/mermaid/
    // maplibre/docx…)是 36MB 的巨型子树,且每次启动都无缓存重爬。exclude
    // 让扫描把该入口 external 化、整棵子树不再爬;代价是 dev 下这些库不预
    // 打包、按原始 ESM 提供(仅影响富预览首次打开速度;生产构建不走
    // optimizeDeps,不受影响)。
    exclude: ['@file-viewer/vue3-full'],
    // 服务器在扫描结束即监听端口,依赖重打包转后台——缓存未命中时也不阻塞
    // tauri 等待 devUrl。
    holdUntilCrawlEnd: false,
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    // Vite 8 自带 Oxc；esbuild 已是可选依赖，干净安装不保证存在。
    // Oxc 保留 maplibre 的 BigInt 字面量，map/shp 预览仍需 Safari 14+。
    minify: !process.env.TAURI_ENV_DEBUG ? 'oxc' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    // The rich-preview viewer (`@file-viewer/vue3-full`) is a lazy chunk that
    // the vite-plugin splits further into per-renderer chunks (pdfjs, three,
    // mermaid, …), fetched only when a user previews that file type — never in
    // the initial load path. Raise the size-warning ceiling above those
    // on-demand chunks so the warning stays meaningful for the eager bundle
    // instead of crying wolf.
    chunkSizeWarningLimit: 1400,
    rolldownOptions: {
      // The file-viewer plugin pins CodeMirror and every renderer into named
      // code-splitting groups, and rolldown by default also captures each
      // grouped module's dependencies. That pulled Vite's preload helper (used
      // by every `import()` in main.ts) into the 1.6 MB vendor-codemirror chunk,
      // and on the safari13 target the class-field helper into the PDF chunk,
      // so every window loaded and evaluated them at startup. Keep captures
      // shallow and give the tiny shared runtime helpers their own chunk; the
      // two companion options are what rolldown recommends when recursive
      // capture is off.
      preserveEntrySignatures: 'allow-extension',
      output: {
        strictExecutionOrder: true,
        codeSplitting: {
          includeDependenciesRecursively: false,
          groups: [
            {
              name: 'app-runtime',
              test: (id: string) => id.startsWith('\0vite/') || /[\\/]@oxc-project[\\/]runtime[\\/]/.test(id),
              priority: 1000,
            },
          ],
        },
      },
    },
  },
});
