<template>
  <section class="plugin-settings-panel" :aria-label="tr('插件设置')" :aria-busy="loading">
    <div class="plugin-toolbar">
      <span class="plugin-count">{{ tr("已安装") }} <strong>{{ packages.length.toLocaleString() }}</strong> {{ tr("个插件包") }}</span>
      <div class="plugin-toolbar-actions">
        <button class="ui-btn" type="button" :disabled="loading || busy !== null" @click="installPackage">
          <Loader2 v-if="busy === INSTALLING" :size="14" class="spin" /><PackagePlus v-else :size="14" aria-hidden="true" />{{ tr("安装插件包…") }}
        </button>
        <button class="ui-icon-btn" type="button" :disabled="loading || busy !== null" :aria-label="tr('刷新插件列表')" :title="tr('刷新')" @click="refresh">
          <Loader2 v-if="loading" :size="14" class="spin" /><RefreshCw v-else :size="14" aria-hidden="true" />
        </button>
      </div>
    </div>
    <p class="plugin-hint">{{ installDirectory ? tr("内置插件包随程序附带；安装的 .lcpkg 插件包保存在 {0}。", { 0: installDirectory }) : tr("内置插件包随程序附带。") }}</p>
    <p v-if="feedback" class="ui-alert" :class="feedback.failed ? 'danger' : 'success'" :role="feedback.failed ? 'alert' : 'status'">{{ feedback.message }}</p>
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
      <p class="ui-empty-desc">{{ tr("安装 .lcpkg 插件包后，它的分析器会出现在快速分析与文件分析中。") }}</p>
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
              <span v-for="filesystem in item.filesystems" :key="filesystem" class="ui-badge">{{ tr("文件系统") }} · {{ filesystem }}</span>
            </div>
            <dl class="plugin-details">
              <dt>{{ tr("标识") }}</dt><dd>{{ item.id }}</dd>
              <dt>{{ tr("记录类型") }}</dt><dd>{{ item.records.length ? joinList(item.records.map(record => catalog.text(item.id, pluginTextKey.record(record.recordType), record.label)), tr) : tr("暂无") }}</dd>
              <dt>{{ tr("权限") }}</dt><dd>{{ item.permissions.length ? joinList(item.permissions.map(permissionLabel), tr) : tr("无需额外权限") }}</dd>
              <dt>{{ tr("内存上限") }}</dt><dd>{{ formatBytes(item.memoryLimit) }}</dd>
              <dt>{{ tr("语言包") }}</dt><dd>{{ Object.keys(item.locales).length ? Object.keys(item.locales).join(' ') : tr("暂无") }}</dd>
              <dt>SDK</dt><dd>{{ item.sdk ?? tr("暂无") }}</dd>
              <dt>{{ tr("结构哈希") }}</dt><dd>{{ item.schemaHash ?? tr("暂无") }}</dd>
              <dt>{{ tr("代码指纹") }}</dt><dd>{{ item.fingerprint || tr("暂无") }}</dd>
              <dt>{{ tr("安装目录") }}</dt><dd>{{ item.directory }}</dd>
              <dt>{{ tr("库文件") }}</dt><dd>{{ item.library }}</dd>
            </dl>
          </div>
        </details>
      </article>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { joinList } from '../../../i18n/translate';

/** Plugin packages: the bundled and installed ones, packages that failed to
 *  load, installing a `.lcpkg` and removing an installed package. */
import { onBeforeUnmount, ref } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import {
  ChevronRight, CircleAlert, Loader2, PackagePlus, Puzzle, RefreshCw, RotateCw, Trash2, TriangleAlert,
} from 'lucide-vue-next';
import { confirm } from '../../../composables/useConfirm';
import { usePluginCatalog } from '../../tasks/api';
import { PLUGIN_PACKAGE_EXTENSION, type AnalyzerScope, type PluginPackageInfo, type PluginPermission } from '../../../lib/ipc/plugins';
import { pluginTextKey } from '../../../lib/models/pluginText';
import { formatBytes } from '../../../utils/format';

const INSTALLING = '\u0000install';
const catalog = usePluginCatalog();
const { packages, rejected, installDirectory, loading, error, refresh } = catalog;
const busy = ref<string | null>(null);
const feedback = ref<{ failed: boolean; message: string } | null>(null);
let disposed = false;
onBeforeUnmount(() => { disposed = true; });

function packageName(item: PluginPackageInfo): string {
  return catalog.text(item.id, pluginTextKey.plugin(), item.name);
}

async function run(id: string, action: () => Promise<string | null>, failure: string) {
  if (busy.value !== null) return;
  busy.value = id;
  feedback.value = null;
  try {
    const message = await action();
    if (!disposed && message) feedback.value = { failed: false, message };
  } catch (cause) {
    if (!disposed) feedback.value = { failed: true, message: tr("{0}：{1}", { 0: failure, 1: String(cause) }) };
  } finally {
    busy.value = null;
  }
}

function installPackage() {
  void run(INSTALLING, async () => {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: tr("插件包"), extensions: [PLUGIN_PACKAGE_EXTENSION] }] });
    if (typeof selected !== 'string') return null;
    const installed = await catalog.install(selected);
    return installed.replaced
      ? tr("已更新 {0}：{1} → {2}", { 0: installed.pluginId, 1: installed.replaced, 2: installed.version })
      : tr("已安装 {0} {1}", { 0: installed.pluginId, 1: installed.version });
  }, tr("安装失败"));
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

function scopeLabel(scope: AnalyzerScope) {
  const labels: Record<AnalyzerScope, string> = {
    system: tr("系统"),
    evidence: tr("整个检材"),
    files: tr("文件"),
  };
  return labels[scope];
}

function permissionLabel(permission: PluginPermission) {
  const labels: Record<PluginPermission, string> = {
    file_materialize: tr("临时物化文件"),
    network: tr("网络访问"),
  };
  return labels[permission];
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
.rejected { display: block; }
.rejected > summary { display: flex; align-items: center; gap: 6px; cursor: pointer; list-style: none; }
.rejected > summary::-webkit-details-marker { display: none; }
.rejected-list { margin-top: 8px; }
.plugin-list { display: grid; gap: 12px; }
.plugin-card { padding: 12px 16px; }
.plugin-card-heading { display: flex; align-items: center; gap: 8px; }
.plugin-icon { display: grid; place-items: center; flex-shrink: 0; width: 32px; height: 32px; margin-right: 4px; border-radius: var(--radius-md); background: var(--sev-info-bg); color: var(--primary-color); }
.plugin-title { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 4px; }
.plugin-title strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 600; }
.plugin-title span { color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; }
.plugin-feedback { margin: 12px 0 0; overflow-wrap: anywhere; }
.plugin-disclosure { margin-top: 8px; font-size: 11px; color: var(--text-secondary); }
.plugin-disclosure > summary { width: fit-content; min-height: 26px; border-radius: var(--radius-sm); }
.plugin-disclosure > summary:hover { color: var(--primary-color); }
.plugin-disclosure > summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.plugin-detail-content { display: flex; flex-direction: column; gap: 12px; padding-top: 12px; }
.analyzer-list { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 0; list-style: none; }
.analyzer-list li { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.analyzer-list strong { color: var(--text-primary); font-size: 12px; font-weight: 500; }
.capability-list { display: flex; flex-wrap: wrap; gap: 6px; }
.plugin-details { display: grid; grid-template-columns: 64px minmax(0, 1fr); gap: 8px 12px; margin: 0; font-size: 11px; line-height: 1.5; }
.plugin-details dd { margin: 0; color: var(--text-primary); font-family: var(--font-mono); overflow-wrap: anywhere; user-select: text; }
@container settings (max-width: 520px) {
  .plugin-card-heading { flex-wrap: wrap; }
  .plugin-title { flex-basis: calc(100% - 40px); }
}
</style>
