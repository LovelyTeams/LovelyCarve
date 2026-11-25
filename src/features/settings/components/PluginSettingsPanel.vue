<template>
  <section class="plugin-settings-panel" :aria-label="tr('插件设置')" :aria-busy="loading">
    <div class="plugin-toolbar">
      <span class="plugin-count">{{ tr("已安装") }} <strong>{{ packages.length.toLocaleString() }}</strong> {{ tr("个插件包") }}</span>
      <div class="plugin-toolbar-actions">
        <button class="ui-btn" type="button" :disabled="loading || busy !== null" @click="choosePackage">
          <Loader2 v-if="busy === INSPECTING" :size="14" class="spin" /><PackagePlus v-else :size="14" aria-hidden="true" />{{ tr("安装插件包…") }}
        </button>
        <button class="ui-icon-btn" type="button" :disabled="loading || busy !== null" :aria-label="tr('刷新插件列表')" :title="tr('刷新')" @click="refresh">
          <Loader2 v-if="loading" :size="14" class="spin" /><RefreshCw v-else :size="14" aria-hidden="true" />
        </button>
      </div>
    </div>
    <p v-if="installDirectory" class="plugin-hint">{{ tr("安装目录：{0}", { 0: installDirectory }) }}</p>
    <div v-if="feedback" class="ui-alert" :class="feedback.failed ? 'danger' : 'success'" :role="feedback.failed ? 'alert' : 'status'">
      <div class="feedback-text"><strong>{{ feedback.title }}</strong><span v-if="feedback.detail">{{ feedback.detail }}</span></div>
    </div>
    <div v-if="error" class="ui-alert danger" role="alert">
      <CircleAlert :size="16" aria-hidden="true" />
      <span class="error-text">{{ tr("读取插件失败：{0}", { 0: error }) }}</span>
      <button class="ui-btn sm" type="button" :disabled="loading || busy !== null" @click="refresh">{{ tr("重试") }}</button>
    </div>
    <details v-if="rejected.length" v-disclosure-motion class="ui-alert warn rejected">
      <summary><TriangleAlert :size="14" aria-hidden="true" />{{ tr("{0} 个插件包未能加载", { 0: rejected.length }) }}</summary>
      <dl class="plugin-details rejected-list">
        <template v-for="item in rejected" :key="item.directory"><dt>{{ tr("目录") }}</dt><dd>{{ item.directory }}</dd><dt>{{ tr("原因") }}</dt><dd>{{ item.error }}</dd></template>
      </dl>
    </details>
    <div v-if="loading && packages.length === 0" class="ui-empty plugin-state" role="status">
      <span class="ui-empty-icon accent"><Loader2 :size="24" class="spin" /></span>
      <p class="ui-empty-desc">{{ tr("正在读取插件…") }}</p>
    </div>
    <div v-else-if="packages.length === 0 && !error" class="ui-empty plugin-state">
      <span class="ui-empty-icon"><Puzzle :size="28" aria-hidden="true" /></span>
      <p class="ui-empty-title">{{ tr("未发现插件") }}</p>
    </div>
    <div v-else class="plugin-list">
      <article v-for="item in packages" :key="item.id" class="ui-card plugin-card">
        <div class="plugin-card-heading">
          <span class="plugin-icon"><Puzzle :size="18" aria-hidden="true" /></span>
          <div class="plugin-title"><strong :title="packageName(item)">{{ packageName(item) }}</strong><span>v{{ item.version }}</span></div>
          <span class="ui-badge">{{ item.removable ? tr("已安装") : tr("内置") }}</span>
          <span v-if="!item.libraryPresent" class="ui-badge danger">{{ tr("缺少库文件") }}</span>
          <span v-else-if="item.filesystems.length" class="ui-badge" :class="stateClass(item)">{{ stateLabel(item) }}</span>
          <button v-if="item.filesystems.length" class="ui-btn sm ghost" type="button" :disabled="loading || busy !== null" @click="restartPackage(item)">
            <Loader2 v-if="busy === item.id" :size="13" class="spin" /><RotateCw v-else :size="13" aria-hidden="true" />{{ tr("重置") }}
          </button>
          <button v-if="item.removable" class="ui-btn sm ghost" type="button" :disabled="loading || busy !== null" @click="uninstallPackage(item)">
            <Trash2 :size="13" aria-hidden="true" />{{ tr("卸载") }}
          </button>
        </div>
        <p v-if="item.description" class="plugin-description">{{ catalog.text(item.id, pluginTextKey.description(), item.description) }}</p>
        <p v-if="item.error" class="ui-alert danger plugin-feedback" role="alert">{{ item.error }}</p>
        <details v-disclosure-motion class="ui-disclosure plugin-disclosure">
          <summary><ChevronRight :size="14" class="ui-chevron" aria-hidden="true" />{{ tr("详情") }}</summary>
          <div class="plugin-detail-content">
            <ul v-if="item.analyzers.length" class="analyzer-list">
              <li v-for="analyzer in item.analyzers" :key="analyzer.id">
                <strong>{{ catalog.text(item.id, pluginTextKey.analyzer(analyzer.id), analyzer.name) }}</strong>
                <span class="ui-badge sm">{{ scopeLabel(analyzer.scope) }}</span>
                <span v-if="analyzer.platforms.length" class="ui-badge sm mono">{{ analyzer.platforms.join(' ') }}</span>
                <span class="ui-badge sm">{{ analyzer.trigger === 'auto' ? tr("自动") : tr("手动") }}</span>
              </li>
            </ul>
            <div v-if="item.filesystems.length" class="capability-list">
              <span v-for="filesystem in item.filesystems" :key="filesystem" class="ui-badge">{{ tr("文件系统") }} {{ filesystem }}</span>
            </div>
            <dl class="plugin-details">
              <dt>{{ tr("标识") }}</dt><dd>{{ item.id }}</dd>
              <template v-if="item.publisher"><dt>{{ tr("发布者") }}</dt><dd>{{ item.publisher }}</dd></template>
              <template v-if="item.license"><dt>{{ tr("许可证") }}</dt><dd>{{ item.license }}</dd></template>
              <dt>{{ tr("记录类型") }}</dt><dd>{{ recordTypeLabels(item) }}</dd>
              <dt>{{ tr("权限") }}</dt><dd>{{ item.permissions.length ? joinList(item.permissions.map(permission => permissionLabel(permission, tr)), tr) : tr("无需额外权限") }}</dd>
              <dt>{{ tr("内存上限") }}</dt><dd>{{ formatBytes(item.memoryLimit) }}</dd>
              <dt>{{ tr("语言包") }}</dt><dd>{{ Object.keys(item.locales).length ? Object.keys(item.locales).join(' ') : tr("暂无") }}</dd>
              <dt>SDK</dt><dd>{{ item.sdk ?? tr("暂无") }}</dd>
              <dt>{{ tr("结构哈希") }}</dt><dd>{{ item.schemaHash ?? tr("暂无") }}</dd>
              <dt>{{ tr("代码指纹") }}</dt><dd>{{ item.fingerprint || tr("暂无") }}</dd>
              <dt>{{ tr("安装目录") }}</dt><dd>{{ item.directory }}</dd>
              <dt>{{ tr("库文件") }}</dt><dd>{{ item.library }}</dd>
            </dl>
            <details v-if="item.hasReadme" v-disclosure-motion class="ui-disclosure readme-disclosure" @toggle="onReadmeToggle(item, $event)">
              <summary><ChevronRight :size="14" class="ui-chevron" aria-hidden="true" />{{ tr("说明文档") }}</summary>
              <div v-if="readmes[item.id]?.state === 'loading'" class="readme-state" role="status"><Loader2 :size="14" class="spin" />{{ tr("正在读取说明文档…") }}</div>
              <p v-else-if="readmes[item.id]?.state === 'failed'" class="ui-alert danger" role="alert">{{ readmes[item.id]?.text }}</p>
              <div v-else-if="readmes[item.id]?.state === 'ready'" class="readme" v-html="readmes[item.id]?.html"></div>
            </details>
          </div>
        </details>
      </article>
    </div>
    <PluginInstallDialog :inspection="pending" :installing="busy === INSTALLING" :failure="installFailure"
      @confirm="confirmInstall" @close="closeInstall" />
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { joinList } from '../../../i18n/translate';

/** Plugin packages: the bundled and installed ones, packages that failed to
 *  load, inspecting and installing a `.lcpkg` and removing an installed
 *  package. */
import { onBeforeUnmount, reactive, ref, shallowRef } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import {
  ChevronRight, CircleAlert, Loader2, PackagePlus, Puzzle, RefreshCw, RotateCw, Trash2, TriangleAlert,
} from 'lucide-vue-next';
import { confirm } from '../../../composables/useConfirm';
import { usePluginCatalog } from '../../tasks/api';
import { renderMarkdownPreview } from '../../preview/api';
import { PLUGIN_PACKAGE_EXTENSION, type AnalyzerScope, type PackageInspection, type PluginPackageInfo } from '../../../lib/ipc/plugins';
import { pluginTextKey } from '../../../lib/models/pluginText';
import type { SafeHtmlString } from '../../../utils/safeHtml';
import { formatBytes } from '../../../utils/format';
import { permissionLabel, pluginFailure, type PluginFailure } from '../model/pluginPackages';
import PluginInstallDialog from './PluginInstallDialog.vue';

const INSPECTING = '\u0000inspect';
const INSTALLING = '\u0000install';
const catalog = usePluginCatalog();
const { packages, rejected, installDirectory, loading, error, refresh } = catalog;
const busy = ref<string | null>(null);
const feedback = shallowRef<{ failed: boolean; title: string; detail?: string } | null>(null);
const pending = shallowRef<PackageInspection | null>(null);
const installFailure = shallowRef<PluginFailure | null>(null);
type Readme = { state: 'loading' } | { state: 'ready'; html: SafeHtmlString } | { state: 'failed'; text: string };
const readmes = reactive<Record<string, Readme>>({});
let disposed = false;
onBeforeUnmount(() => { disposed = true; });

function packageName(item: PluginPackageInfo): string {
  return catalog.text(item.id, pluginTextKey.plugin(), item.name);
}

function recordTypeLabels(item: PluginPackageInfo): string {
  return item.records.length
    ? joinList(item.records.map(record => catalog.text(item.id, pluginTextKey.record(record.recordType), record.label)), tr)
    : tr("暂无");
}

async function run(id: string, action: () => Promise<string | null>, failure: string) {
  if (busy.value !== null) return;
  busy.value = id;
  feedback.value = null;
  try {
    const message = await action();
    if (!disposed && message) feedback.value = { failed: false, title: message };
  } catch (cause) {
    if (!disposed) feedback.value = { failed: true, ...pluginFailure(cause, failure, tr) };
  } finally {
    busy.value = null;
  }
}

/** Picks a package and inspects it; installing waits for the dialog. */
function choosePackage() {
  void run(INSPECTING, async () => {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: tr("插件包"), extensions: [PLUGIN_PACKAGE_EXTENSION] }] });
    if (typeof selected !== 'string') return null;
    const inspection = await catalog.inspect(selected);
    if (!disposed) {
      installFailure.value = null;
      pending.value = inspection;
    }
    return null;
  }, tr("读取插件包失败"));
}

async function confirmInstall() {
  const inspection = pending.value;
  if (!inspection || busy.value !== null) return;
  busy.value = INSTALLING;
  installFailure.value = null;
  try {
    const installed = await catalog.install(inspection.archive, inspection.sha256);
    if (disposed) return;
    pending.value = null;
    feedback.value = {
      failed: false,
      title: installed.replaced
        ? tr("已更新 {0}：{1} → {2}", { 0: installed.pluginId, 1: installed.replaced, 2: installed.version })
        : tr("已安装 {0} {1}", { 0: installed.pluginId, 1: installed.version }),
    };
  } catch (cause) {
    if (!disposed) installFailure.value = pluginFailure(cause, tr("安装失败"), tr);
  } finally {
    busy.value = null;
  }
}

function closeInstall() {
  pending.value = null;
  installFailure.value = null;
}

async function uninstallPackage(item: PluginPackageInfo) {
  const ok = await confirm({
    title: tr("卸载插件包"),
    message: tr("卸载 {0}？已发布的分析结果保留在案件里。", { 0: packageName(item) }),
    confirmText: tr("卸载"),
    danger: true,
  });
  if (!ok || disposed) return;
  await run(item.id, async () => {
    await catalog.uninstall(item.id);
    return tr("已卸载 {0}", { 0: packageName(item) });
  }, tr("卸载失败"));
}

async function restartPackage(item: PluginPackageInfo) {
  await run(item.id, async () => {
    await catalog.restart(item.id);
    return tr("插件已重置，将在下次使用时启动");
  }, tr("重置失败"));
}

/** Reads a package's README the first time its disclosure opens. */
async function onReadmeToggle(item: PluginPackageInfo, event: Event) {
  if (!(event.target as HTMLDetailsElement).open || readmes[item.id]) return;
  readmes[item.id] = { state: 'loading' };
  try {
    const text = await catalog.readme(item.id);
    if (!disposed) readmes[item.id] = { state: 'ready', html: renderMarkdownPreview(text ?? '') };
  } catch (cause) {
    if (!disposed) readmes[item.id] = { state: 'failed', text: tr("读取说明文档失败：{0}", { 0: String(cause) }) };
  }
}

function scopeLabel(scope: AnalyzerScope) {
  const labels: Record<AnalyzerScope, string> = {
    system: tr("系统"),
    evidence: tr("整个检材"),
    files: tr("文件"),
  };
  return labels[scope];
}

function stateLabel(item: PluginPackageInfo) {
  const labels: Record<PluginPackageInfo['state'], string> = {
    configured: tr("已就绪"),
    starting: tr("启动中"),
    running: tr("运行中"),
    crashed: tr("异常退出"),
  };
  return labels[item.state];
}

function stateClass(item: PluginPackageInfo) {
  if (item.state === 'crashed') return 'danger';
  if (item.state === 'running') return 'success';
  return 'info';
}
</script>

<style scoped>
.plugin-settings-panel { display: flex; flex-direction: column; gap: 12px; }
.plugin-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.plugin-toolbar-actions { display: flex; align-items: center; gap: 6px; }
.plugin-count { color: var(--text-secondary); font-size: 12px; }
.plugin-count strong { color: var(--text-primary); font-family: var(--font-mono); font-weight: 600; }
.plugin-hint { margin: 0; color: var(--text-secondary); font-size: 11px; line-height: 1.5; overflow-wrap: anywhere; }
.plugin-state { min-height: 160px; }
.error-text { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.feedback-text { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 2px; overflow-wrap: anywhere; }
.feedback-text span { font-size: 11px; }
.rejected { display: block; }
.rejected > summary { display: flex; align-items: center; gap: 6px; cursor: pointer; list-style: none; }
.rejected > summary::-webkit-details-marker { display: none; }
.rejected-list { margin-top: 8px; }
.plugin-list { display: grid; gap: 12px; }
.plugin-card { padding: 12px 16px; }
.plugin-card-heading { display: flex; align-items: center; gap: 8px; }
.plugin-icon { display: grid; place-items: center; flex-shrink: 0; width: 32px; height: 32px; margin-right: 4px; border-radius: var(--radius-md); background: var(--primary-soft); color: var(--primary-color); }
.plugin-title { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 4px; }
.plugin-title strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 600; }
.plugin-title span { color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; }
.plugin-description { margin: 8px 0 0 44px; color: var(--text-secondary); font-size: 12px; line-height: 1.5; overflow-wrap: anywhere; }
.plugin-feedback { margin: 12px 0 0; overflow-wrap: anywhere; }
.plugin-disclosure { margin-top: 8px; font-size: 11px; color: var(--text-secondary); }
.plugin-disclosure > summary,
.readme-disclosure > summary { width: fit-content; min-height: 26px; border-radius: var(--radius-sm); }
.plugin-disclosure > summary:hover,
.readme-disclosure > summary:hover { color: var(--primary-color); }
.plugin-disclosure > summary:focus-visible,
.readme-disclosure > summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.plugin-detail-content { display: flex; flex-direction: column; gap: 12px; padding-top: 12px; }
.analyzer-list { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 0; list-style: none; }
.analyzer-list li { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.analyzer-list strong { color: var(--text-primary); font-size: 12px; font-weight: 500; }
.capability-list { display: flex; flex-wrap: wrap; gap: 6px; }
.plugin-details { display: grid; grid-template-columns: 64px minmax(0, 1fr); gap: 8px 12px; margin: 0; font-size: 11px; line-height: 1.5; }
.plugin-details dd { margin: 0; color: var(--text-primary); font-family: var(--font-mono); overflow-wrap: anywhere; user-select: text; }
.readme-state { display: flex; align-items: center; gap: 6px; padding-top: 8px; }
.readme { max-height: 360px; margin-top: 8px; padding: 12px 16px; overflow: auto; border-radius: var(--radius-md); background: var(--surface-tertiary); color: var(--text-primary); font-size: 12px; line-height: 1.6; overflow-wrap: anywhere; user-select: text; }
.readme :deep(.md-heading) { margin: 12px 0 6px; font-size: 13px; font-weight: 600; }
.readme :deep(.md-h1) { font-size: 15px; }
.readme :deep(p), .readme :deep(.md-list), .readme :deep(.md-quote) { margin: 6px 0; }
.readme :deep(.md-list) { padding-left: 20px; }
.readme :deep(.md-quote) { padding-left: 10px; border-left: 2px solid var(--border-color); color: var(--text-secondary); }
.readme :deep(.md-inline-code), .readme :deep(.md-code-block) { font-family: var(--font-mono); font-size: 11px; }
.readme :deep(.md-code-block) { margin: 6px 0; padding: 8px 10px; overflow-x: auto; border-radius: var(--radius-sm); background: var(--surface-secondary); white-space: pre; }
.readme :deep(.md-table) { border-collapse: collapse; margin: 6px 0; }
.readme :deep(.md-table th), .readme :deep(.md-table td) { padding: 4px 8px; border: 1px solid var(--border-color); text-align: left; }
.readme :deep(a) { color: var(--primary-color); }
@container settings (max-width: 520px) {
  .plugin-card-heading { flex-wrap: wrap; }
  .plugin-title { flex-basis: calc(100% - 40px); }
  .plugin-description { margin-left: 0; }
}
</style>
