<template>
  <section class="plugin-settings-panel" aria-label="插件设置" :aria-busy="loading || checking">
    <div class="plugin-toolbar">
      <span class="plugin-count">已安装 <strong>{{ plugins.length.toLocaleString() }}</strong> 个插件</span>
      <button class="ui-btn" type="button" :disabled="loading || checking || busyPluginId !== null" @click="rescanPlugins">
        <Loader2 v-if="checking" :size="14" class="spin" /><RefreshCw v-else :size="14" aria-hidden="true" />{{ checking ? '检查中' : '检查插件目录' }}
      </button>
    </div>
    <p class="plugin-hint">放入完整插件目录后点击检查；展开详情查看能力、权限与安装位置。</p>
    <p v-if="scanSummary" class="ui-alert success" role="status">{{ scanSummary }}</p>
    <p v-if="subscriptionError" class="ui-alert warn" role="alert">{{ subscriptionError }}</p>
    <details v-if="report" v-disclosure-motion class="ui-disclosure plugin-disclosure">
      <summary><ChevronRight :size="14" class="ui-chevron" aria-hidden="true" />检查目录与诊断（{{ diagnostics.length }}）</summary>
      <div class="plugin-detail-content">
        <p v-for="root in report.roots" :key="root" class="plugin-path">{{ root }}</p>
        <div v-for="(diagnostic, index) in diagnostics" :key="index" class="ui-alert plugin-feedback" :class="diagnostic.status === 'error' ? 'danger' : 'warn'">
          <div><strong>{{ diagnostic.pluginId || '目录检查' }}</strong><p class="plugin-path">{{ diagnostic.directory }}</p><p class="plugin-hint">{{ diagnostic.message }}<template v-if="diagnostic.status === 'updatePending' && diagnostic.version">（扫描版本 {{ diagnostic.version }}）</template></p></div>
        </div>
      </div>
    </details>
    <div v-if="error" class="ui-alert danger" role="alert">
      <CircleAlert :size="16" aria-hidden="true" />
      <span class="error-text">读取插件失败：{{ error }}</span>
      <button class="ui-btn sm" type="button" :disabled="loading || checking || busyPluginId !== null" @click="loadPlugins">重试</button>
    </div>
    <div v-if="loading && plugins.length === 0" class="ui-empty plugin-state" role="status">
      <span class="ui-empty-icon accent"><Loader2 :size="24" class="spin" /></span>
      <p class="ui-empty-desc">正在读取插件…</p>
    </div>
    <div v-else-if="plugins.length === 0 && !error" class="ui-empty plugin-state">
      <span class="ui-empty-icon"><Puzzle :size="28" aria-hidden="true" /></span>
      <p class="ui-empty-title">未发现插件</p>
      <p class="ui-empty-desc">将插件清单与动态库放入应用数据目录的 plugins 文件夹，然后点击“检查插件目录”。</p>
    </div>
    <div v-else class="plugin-list">
      <article v-for="plugin in plugins" :key="plugin.id" class="ui-card plugin-card">
        <div class="plugin-card-heading">
          <span class="plugin-icon"><Puzzle :size="18" aria-hidden="true" /></span>
          <div class="plugin-title"><strong :title="plugin.name">{{ plugin.name }}</strong><span>v{{ plugin.version }}</span></div>
          <span v-if="report?.entries.some(entry => entry.pluginId === plugin.id && entry.status === 'updatePending')" class="ui-badge warn">更新待重启</span>
          <span class="ui-badge" :class="stateClass(plugin)">{{ stateLabel(plugin) }}</span>
          <button class="ui-btn sm ghost" type="button" :disabled="loading || checking || busyPluginId !== null" @click="restartPlugin(plugin)">
            <Loader2 v-if="busyPluginId === plugin.id" :size="13" class="spin" /><RotateCw v-else :size="13" aria-hidden="true" />
            {{ busyPluginId === plugin.id ? '重置中' : '重置' }}
          </button>
        </div>
        <p v-if="plugin.error" class="ui-alert danger plugin-feedback" role="alert">{{ plugin.error }}</p>
        <p v-if="actionFeedback?.pluginId === plugin.id" class="ui-alert plugin-feedback" :class="actionFeedback.failed ? 'danger' : 'success'" :role="actionFeedback.failed ? 'alert' : 'status'">{{ actionFeedback.message }}</p>
        <details v-disclosure-motion class="ui-disclosure plugin-disclosure">
          <summary><ChevronRight :size="14" class="ui-chevron" aria-hidden="true" />详情</summary>
          <div class="plugin-detail-content">
            <div class="capability-list">
              <span v-for="capability in plugin.capabilities" :key="`${capability.kind}:${capability.name}`" class="ui-badge">{{ capabilityLabel(capability.kind) }} · {{ capability.name }}</span>
              <span v-if="plugin.capabilities.length === 0" class="ui-badge">未声明可用能力</span>
            </div>
            <dl class="plugin-details">
              <dt>标识</dt><dd>{{ plugin.id }}</dd>
              <dt>加载方式</dt><dd>{{ isolationLabel(plugin) }}</dd>
              <dt>权限</dt><dd>{{ plugin.permissions.length ? plugin.permissions.map(permissionLabel).join('、') : '无需额外权限' }}</dd>
              <dt>代码指纹</dt><dd>{{ plugin.fingerprint || '暂无' }}</dd>
              <dt>安装目录</dt><dd>{{ plugin.directory }}</dd>
              <dt>库文件</dt><dd>{{ plugin.library }}</dd>
            </dl>
          </div>
        </details>
      </article>
    </div>
  </section>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import {
  ChevronRight, CircleAlert, Loader2, Puzzle, RefreshCw, RotateCw,
} from 'lucide-vue-next';
import {
  pluginList, pluginRestart, pluginRescan, pluginScanReport, onPluginCatalogChanged,
  type PluginCapabilityInfo, type PluginInfo, type PluginPermission, type PluginScanReport,
} from '../../../lib/ipc/plugins';

const plugins = ref<PluginInfo[]>([]);
const loading = ref(false);
const checking = ref(false);
const report = ref<PluginScanReport | null>(null);
const scanSummary = ref('');
const subscriptionError = ref('');
const diagnostics = computed(() => report.value?.entries.filter(entry => !['added', 'unchanged'].includes(entry.status)) ?? []);
const error = ref('');
const busyPluginId = ref<string | null>(null);
const actionFeedback = ref<{ pluginId: string; failed: boolean; message: string } | null>(null);
let disposed = false;
let requestGeneration = 0;
let unlisten: (() => void) | null = null;
onBeforeUnmount(() => { disposed = true; requestGeneration++; unlisten?.(); });

onMounted(async () => {
  try {
    const stop = await onPluginCatalogChanged(() => {
      if (!checking.value) void loadPlugins();
    });
    if (disposed) { stop(); return; }
    unlisten = stop;
  } catch (cause) {
    if (!disposed) subscriptionError.value = `插件变化订阅失败：${String(cause)}，可手动检查目录。`;
  }
  if (!disposed) void loadPlugins();
});

async function rescanPlugins() {
  if (checking.value || loading.value || busyPluginId.value !== null || disposed) return;
  checking.value = true;
  scanSummary.value = '';
  error.value = '';
  try {
    const result = await pluginRescan();
    if (disposed) return;
    const count = result.entries.filter(entry => entry.status === 'added').length;
    scanSummary.value = `检查完成，新发现 ${count} 个插件。`;
    await loadPlugins();
  } catch (cause) {
    if (!disposed) error.value = `检查目录失败：${String(cause)}`;
  } finally {
    checking.value = false;
  }
}

async function loadPlugins() {
  if (disposed) return;
  const generation = ++requestGeneration;
  loading.value = true;
  error.value = '';
  try {
    const [result, scan] = await Promise.all([pluginList(), pluginScanReport()]);
    if (!disposed && generation === requestGeneration) {
      plugins.value = result;
      report.value = scan;
    }
  } catch (cause) {
    if (!disposed && generation === requestGeneration) error.value = String(cause);
  } finally {
    if (generation === requestGeneration) loading.value = false;
  }
}

async function restartPlugin(plugin: PluginInfo) {
  if (loading.value || busyPluginId.value !== null) return;
  await runAction(plugin, () => pluginRestart(plugin.id), '插件已重置，将在下次使用时启动');
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
    if (!disposed) actionFeedback.value = { pluginId: plugin.id, failed: true, message: `重置失败：${String(cause)}。请重试。` };
  } finally {
    busyPluginId.value = null;
  }
}

function permissionLabel(permission: PluginPermission) {
  const labels: Record<PluginPermission, string> = {
    file_materialize: '临时物化文件',
    emit_artifacts: '写入结构化结果',
    network: '网络访问',
  };
  return labels[permission];
}

function capabilityLabel(kind: PluginCapabilityInfo['kind']) {
  const labels: Record<PluginCapabilityInfo['kind'], string> = {
    filesystem: '文件系统',
    fileAnalyzer: '文件分析',
    evidenceAnalyzer: '检材分析',
  };
  return labels[kind];
}

function isolationLabel(plugin: PluginInfo) {
  return plugin.isolation === 'inProcess' ? '进程内（无隔离）' : '独立进程（lc-plugin-host）';
}

function stateLabel(plugin: PluginInfo) {
  const labels: Record<PluginInfo['state'], string> = {
    configured: '已就绪',
    starting: '启动中',
    running: '运行中',
    crashed: '异常退出',
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
.plugin-path { margin: 4px 0; font-family: var(--font-mono); font-size: 11px; color: var(--text-secondary); overflow-wrap: anywhere; user-select: text; }
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
