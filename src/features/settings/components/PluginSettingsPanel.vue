<template>
  <section class="plugin-settings-panel" :aria-label="tr('插件设置')" :aria-busy="loading">
    <div class="plugin-toolbar">
      <span class="plugin-count">{{ tr("已安装") }} <strong>{{ plugins.length.toLocaleString() }}</strong> {{ tr("个插件") }}</span>
      <button class="ui-btn" type="button" :disabled="loading || busyPluginId !== null" @click="loadPlugins">
        <Loader2 v-if="loading" :size="14" class="spin" /><RefreshCw v-else :size="14" aria-hidden="true" />{{ tr("刷新") }}
      </button>
    </div>
    <p class="plugin-hint">{{ tr("查看插件状态；展开详情查看能力、权限与安装位置。") }}</p>
    <div v-if="error" class="ui-alert danger" role="alert">
      <CircleAlert :size="16" aria-hidden="true" />
      <span class="error-text">{{ tr("读取插件失败：{0}", { 0: error }) }}</span>
      <button class="ui-btn sm" type="button" :disabled="loading || busyPluginId !== null" @click="loadPlugins">{{ tr("重试") }}</button>
    </div>
    <div v-if="loading && plugins.length === 0" class="ui-empty plugin-state" role="status">
      <span class="ui-empty-icon accent"><Loader2 :size="24" class="spin" /></span>
      <p class="ui-empty-desc">{{ tr("正在读取插件…") }}</p>
    </div>
    <div v-else-if="plugins.length === 0 && !error" class="ui-empty plugin-state">
      <span class="ui-empty-icon"><Puzzle :size="28" aria-hidden="true" /></span>
      <p class="ui-empty-title">{{ tr("未发现插件") }}</p>
      <p class="ui-empty-desc">{{ tr("将插件清单与动态库放入应用数据目录的 plugins 文件夹，然后刷新。") }}</p>
    </div>
    <div v-else class="plugin-list">
      <article v-for="plugin in plugins" :key="plugin.id" class="ui-card plugin-card">
        <div class="plugin-card-heading">
          <span class="plugin-icon"><Puzzle :size="18" aria-hidden="true" /></span>
          <div class="plugin-title"><strong :title="plugin.name">{{ plugin.name }}</strong><span>v{{ plugin.version }}</span></div>
          <span class="ui-badge" :class="stateClass(plugin)">{{ stateLabel(plugin) }}</span>
          <button class="ui-btn sm ghost" type="button" :disabled="loading || busyPluginId !== null" @click="restartPlugin(plugin)">
            <Loader2 v-if="busyPluginId === plugin.id" :size="13" class="spin" /><RotateCw v-else :size="13" aria-hidden="true" />
            {{ busyPluginId === plugin.id ? tr("重置中") : tr("重置") }}
          </button>
        </div>
        <p v-if="plugin.error" class="ui-alert danger plugin-feedback" role="alert">{{ plugin.error }}</p>
        <p v-if="actionFeedback?.pluginId === plugin.id" class="ui-alert plugin-feedback" :class="actionFeedback.failed ? 'danger' : 'success'" :role="actionFeedback.failed ? 'alert' : 'status'">{{ actionFeedback.message }}</p>
        <details v-disclosure-motion class="ui-disclosure plugin-disclosure">
          <summary><ChevronRight :size="14" class="ui-chevron" aria-hidden="true" />{{ tr("详情") }}</summary>
          <div class="plugin-detail-content">
            <div class="capability-list">
              <span v-for="capability in plugin.capabilities" :key="`${capability.kind}:${capability.name}`" class="ui-badge">{{ capabilityLabel(capability.kind) }} · {{ capability.name }}</span>
              <span v-if="plugin.capabilities.length === 0" class="ui-badge">{{ tr("未声明可用能力") }}</span>
            </div>
            <dl class="plugin-details">
              <dt>{{ tr("标识") }}</dt><dd>{{ plugin.id }}</dd>
              <dt>{{ tr("加载方式") }}</dt><dd>{{ isolationLabel(plugin) }}</dd>
              <dt>{{ tr("权限") }}</dt><dd>{{ plugin.permissions.length ? joinList(plugin.permissions.map(permissionLabel), tr) : tr("无需额外权限") }}</dd>
              <dt>{{ tr("代码指纹") }}</dt><dd>{{ plugin.fingerprint || tr("暂无") }}</dd>
              <dt>{{ tr("安装目录") }}</dt><dd>{{ plugin.directory }}</dd>
              <dt>{{ tr("库文件") }}</dt><dd>{{ plugin.library }}</dd>
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

import { onBeforeUnmount, onMounted, ref } from 'vue';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import {
  ChevronRight, CircleAlert, Loader2, Puzzle, RefreshCw, RotateCw,
} from 'lucide-vue-next';
import {
  pluginList, pluginRestart,
  type PluginCapabilityInfo, type PluginInfo, type PluginPermission,
} from '../../../lib/ipc/plugins';

const plugins = ref<PluginInfo[]>([]);
const loading = ref(false);
const error = ref('');
const busyPluginId = ref<string | null>(null);
const actionFeedback = ref<{ pluginId: string; failed: boolean; message: string } | null>(null);
let disposed = false;
onBeforeUnmount(() => { disposed = true; });

onMounted(() => void loadPlugins());

async function loadPlugins() {
  if (loading.value || disposed) return;
  loading.value = true;
  error.value = '';
  try {
    const result = await pluginList();
    if (!disposed) plugins.value = result;
  } catch (cause) {
    if (!disposed) error.value = String(cause);
  } finally {
    loading.value = false;
  }
}

async function restartPlugin(plugin: PluginInfo) {
  if (loading.value || busyPluginId.value !== null) return;
  await runAction(plugin, () => pluginRestart(plugin.id), tr("插件已重置，将在下次使用时启动"));
}

async function runAction(plugin: PluginInfo, action: () => Promise<void>, success: string) {
  busyPluginId.value = plugin.id;
  actionFeedback.value = null;
  try {
    await action();
    if (disposed) return;
    actionFeedback.value = { pluginId: plugin.id, failed: false, message: success };
    await loadPlugins();
  } catch (cause) {
    if (!disposed) actionFeedback.value = { pluginId: plugin.id, failed: true, message: tr("重置失败：{0}。请重试。", { 0: String(cause) }) };
  } finally {
    busyPluginId.value = null;
  }
}

function permissionLabel(permission: PluginPermission) {
  const labels: Record<PluginPermission, string> = {
    file_materialize: tr("临时物化文件"),
    emit_artifacts: tr("写入结构化结果"),
    network: tr("网络访问"),
  };
  return labels[permission];
}

function capabilityLabel(kind: PluginCapabilityInfo['kind']) {
  const labels: Record<PluginCapabilityInfo['kind'], string> = {
    filesystem: tr("文件系统"),
    fileAnalyzer: tr("文件分析"),
    evidenceAnalyzer: tr("检材分析"),
  };
  return labels[kind];
}

function isolationLabel(plugin: PluginInfo) {
  return plugin.isolation === 'inProcess' ? tr("进程内（无隔离）") : tr("独立进程（lc-plugin-host）");
}

function stateLabel(plugin: PluginInfo) {
  const labels: Record<PluginInfo['state'], string> = {
    configured: tr("已就绪"),
    starting: tr("启动中"),
    running: tr("运行中"),
    crashed: tr("异常退出"),
  };
  return labels[plugin.state];
}

function stateClass(plugin: PluginInfo) {
  if (plugin.state === 'crashed') return 'danger';
  if (plugin.state === 'running') return 'success';
  return 'info';
}
</script>

<style scoped>
.plugin-settings-panel { display: flex; flex-direction: column; gap: 12px; }
.plugin-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.plugin-count { color: var(--text-secondary); font-size: 12px; }
.plugin-count strong { color: var(--text-primary); font-family: var(--font-mono); font-weight: 600; }
.plugin-hint { margin: 0; color: var(--text-secondary); font-size: 11px; line-height: 1.5; }
.plugin-state { min-height: 160px; }
.error-text { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.plugin-list { display: grid; gap: 12px; }
.plugin-card { padding: 12px 16px; }
.plugin-card-heading { display: flex; align-items: center; gap: 12px; }
.plugin-icon { display: grid; place-items: center; flex-shrink: 0; width: 32px; height: 32px; border-radius: var(--radius-md); background: var(--sev-info-bg); color: var(--primary-color); }
.plugin-title { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 4px; }
.plugin-title strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 600; }
.plugin-title span { color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; }
.plugin-feedback { margin: 12px 0 0; overflow-wrap: anywhere; }
.plugin-disclosure { margin-top: 8px; font-size: 11px; color: var(--text-secondary); }
.plugin-disclosure > summary { width: fit-content; min-height: 26px; border-radius: var(--radius-sm); }
.plugin-disclosure > summary:hover { color: var(--primary-color); }
.plugin-disclosure > summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.plugin-detail-content { padding-top: 12px; }
.capability-list { display: flex; flex-wrap: wrap; gap: 6px; }
.plugin-details { display: grid; grid-template-columns: 64px minmax(0, 1fr); gap: 8px 12px; margin: 16px 0 0; font-size: 11px; line-height: 1.5; }
.plugin-details dd { margin: 0; color: var(--text-primary); font-family: var(--font-mono); overflow-wrap: anywhere; user-select: text; }
@container settings (max-width: 520px) {
  .plugin-card-heading { flex-wrap: wrap; gap: 8px; }
  .plugin-title { flex-basis: calc(100% - 40px); }
}
</style>
