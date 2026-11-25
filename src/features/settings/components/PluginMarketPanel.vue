<template>
  <section class="market-panel" :aria-label="tr('插件市场')" :aria-busy="loadingCatalog">
    <form class="source-form" @submit.prevent="addSource">
      <input v-model="draft" class="ui-input mono source-input" :aria-label="tr('索引源')" :placeholder="tr('https:// 地址或本地市场目录')"
        :aria-invalid="sourceError ? 'true' : undefined" :aria-describedby="sourceError ? sourceErrorId : undefined" @input="sourceError = ''" />
      <button class="ui-icon-btn" type="button" :title="tr('选择本地市场目录')" :aria-label="tr('选择本地市场目录')" :disabled="saving" @click="pickDirectory">
        <FolderOpen :size="14" aria-hidden="true" />
      </button>
      <button class="ui-btn" type="submit" :disabled="saving || !draft.trim()">
        <Loader2 v-if="saving" :size="14" class="spin" /><Plus v-else :size="14" aria-hidden="true" />{{ tr("添加索引源") }}
      </button>
    </form>
    <p v-if="sourceError" :id="sourceErrorId" class="ui-field-error" role="alert">{{ sourceError }}</p>
    <div v-if="sourcesFailure" class="ui-alert danger" role="alert">
      <div class="alert-text"><strong>{{ sourcesFailure.title }}</strong><span>{{ sourcesFailure.detail }}</span></div>
      <button class="ui-btn sm" type="button" @click="loadSources">{{ tr("重试") }}</button>
    </div>
    <div v-if="downloadingLinkedPackage" class="ui-alert" role="status">
      <Loader2 :size="16" class="spin" aria-hidden="true" />{{ tr("正在下载插件包…") }}
    </div>
    <div v-if="feedback" class="ui-alert" :class="feedback.failed ? 'danger' : 'success'" :role="feedback.failed ? 'alert' : 'status'">
      <div class="alert-text"><strong>{{ feedback.title }}</strong><span v-if="feedback.detail">{{ feedback.detail }}</span></div>
    </div>
    <div v-if="loadingSources && !sources.length" class="ui-empty market-state" role="status">
      <span class="ui-empty-icon accent"><Loader2 :size="24" class="spin" /></span>
    </div>
    <div v-else-if="!sources.length" class="ui-empty market-state">
      <span class="ui-empty-icon"><Store :size="28" aria-hidden="true" /></span>
      <p class="ui-empty-title">{{ tr("未配置索引源") }}</p>
    </div>
    <template v-else>
      <div class="market-toolbar">
        <UiSelect v-if="sources.length > 1" v-model="selectedSource" class="source-select" :options="sourceOptions" />
        <span v-else class="source-label mono" :title="selectedSource">{{ selectedSource }}</span>
        <label class="ui-search market-search" :class="{ applied: query }">
          <Search :size="13" aria-hidden="true" />
          <input v-model="query" :aria-label="tr('筛选插件')" :placeholder="tr('筛选插件…')" @keydown.esc.stop.prevent="query = ''" />
          <button v-if="query" class="ui-icon-btn sm" type="button" :title="tr('清除筛选')" :aria-label="tr('清除筛选')" @click="query = ''"><X :size="12" aria-hidden="true" /></button>
        </label>
        <div class="ui-toolbar-actions">
          <button class="ui-icon-btn" type="button" :title="tr('重新读取索引')" :aria-label="tr('重新读取索引')" :disabled="loadingCatalog" @click="loadCatalog(true)">
            <Loader2 v-if="loadingCatalog" :size="14" class="spin" /><RefreshCw v-else :size="14" aria-hidden="true" />
          </button>
          <button class="ui-icon-btn" type="button" :title="tr('移除索引源')" :aria-label="tr('移除索引源')" :disabled="saving || !selected" @click="removeSelected">
            <Trash2 :size="14" aria-hidden="true" />
          </button>
        </div>
      </div>
      <div v-if="catalogFailure" class="ui-alert danger" role="alert">
        <CircleAlert :size="16" aria-hidden="true" />
        <div class="alert-text"><strong>{{ catalogFailure.title }}</strong><span>{{ catalogFailure.detail }}</span></div>
        <button class="ui-btn sm" type="button" :disabled="loadingCatalog" @click="loadCatalog(true)">{{ tr("重试") }}</button>
      </div>
      <div v-if="loadingCatalog && !catalog" class="ui-empty market-state" role="status">
        <span class="ui-empty-icon accent"><Loader2 :size="24" class="spin" /></span>
        <p class="ui-empty-desc">{{ tr("正在读取插件清单…") }}</p>
      </div>
      <template v-else-if="catalog">
        <div class="market-meta">
          <strong>{{ catalog.title }}</strong>
          <span v-if="catalog.description">{{ catalog.description }}</span>
          <span class="ui-badge sm mono">{{ catalog.platform }}</span>
          <span class="ui-badge sm mono" :title="tr('读取时间')">{{ catalog.fetchedAt }}</span>
        </div>
        <div v-if="!visible.length" class="ui-empty market-state">
          <span class="ui-empty-icon"><Puzzle :size="28" aria-hidden="true" /></span>
          <p class="ui-empty-title">{{ catalog.packages.length ? tr("没有匹配的插件") : tr("索引中没有插件") }}</p>
        </div>
        <div v-else class="market-list">
          <article v-for="entry in visible" :key="entry.id" class="ui-card market-card">
            <div class="market-card-heading">
              <span class="market-icon"><Puzzle :size="18" aria-hidden="true" /></span>
              <div class="market-title">
                <strong :title="entryName(entry)">{{ entryName(entry) }}</strong>
                <span>{{ entry.id }}<template v-if="entry.latest"> v{{ entry.latest }}</template></span>
              </div>
              <span v-if="entry.status !== 'notInstalled'" class="ui-badge" :class="statusClass(entry.status)">{{ marketStatusLabel(entry.status, tr) }}</span>
              <button v-if="entry.latest && (entry.status === 'notInstalled' || entry.status === 'updateAvailable')" class="ui-btn sm"
                :class="{ primary: entry.status === 'updateAvailable' }" type="button" :disabled="busy !== null" @click="download(entry, entry.latest)">
                <Loader2 v-if="busy === key(entry.id, entry.latest)" :size="13" class="spin" /><Download v-else :size="13" aria-hidden="true" />{{ entry.status === 'updateAvailable' ? tr("更新") : tr("安装") }}
              </button>
            </div>
            <p v-if="entryDescription(entry)" class="market-description">{{ entryDescription(entry) }}</p>
            <div v-if="entry.publisher || entry.license || entry.installed" class="market-attribution">
              <span v-if="entry.publisher">{{ entry.publisher }}</span>
              <span v-if="entry.license" class="mono">{{ entry.license }}</span>
              <span v-if="entry.installed" class="mono">{{ tr("已安装 {0}", { 0: entry.installed }) }}</span>
            </div>
            <details v-disclosure-motion class="ui-disclosure market-disclosure">
              <summary><ChevronRight :size="14" class="ui-chevron" aria-hidden="true" />{{ tr("{0} 个版本", { 0: entry.versions.length }) }}</summary>
              <ul class="version-list">
                <li v-for="version in entry.versions" :key="version.version">
                  <span class="mono version-number">{{ version.version }}</span>
                  <span class="mono version-date">{{ version.published.slice(0, 10) }}</span>
                  <span v-if="version.prerelease" class="ui-badge sm warn">{{ tr("预发布") }}</span>
                  <span v-if="version.size === null" class="ui-badge sm">{{ tr("当前平台不可用") }}</span>
                  <span v-else class="version-size">{{ formatBytes(version.size) }}</span>
                  <span v-for="permission in version.permissions" :key="permission" class="ui-badge sm warn">{{ permissionLabel(permission, tr) }}</span>
                  <button v-if="version.size !== null && entry.status !== 'bundled'" class="ui-btn sm ghost version-install" type="button"
                    :disabled="busy !== null" @click="download(entry, version.version)">
                    <Loader2 v-if="busy === key(entry.id, version.version)" :size="13" class="spin" /><Download v-else :size="13" aria-hidden="true" />{{ tr("安装此版本") }}
                  </button>
                </li>
              </ul>
            </details>
          </article>
        </div>
      </template>
    </template>
    <PluginInstallDialog :inspection="pending?.inspection ?? null" :source="pending?.source ?? null"
      :installing="busy === INSTALLING" :failure="installFailure" @confirm="confirmInstall" @close="cancelInstall" />
  </section>
</template>

<script setup lang="ts">
import { tr, useLocale } from '../../../composables/useLocale';

/** The plugin market: index sources, the selected source's packages against
 *  what is installed, and installing a version through the same inspection
 *  and confirmation as a local package. */
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, useId, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import {
  ChevronRight, CircleAlert, Download, FolderOpen, Loader2, Plus, Puzzle, RefreshCw, Search, Store, Trash2, X,
} from 'lucide-vue-next';
import UiSelect from '../../../components/ui/UiSelect.vue';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import { notifyError } from '../../../composables/useToast';
import { pluginMarketDownload, pluginMarketDownloadOfficial, pluginMarketRelease, type MarketDownload, type MarketPackage, type MarketStatus } from '../../../lib/ipc/pluginMarket';
import type { PluginInstallLink } from '../../../lib/ipc/evidenceLinks';
import { pluginText, pluginTextKey } from '../../../lib/models/pluginText';
import { formatBytes } from '../../../utils/format';
import { usePluginCatalog } from '../../tasks/api';
import { usePluginMarket } from '../composables/usePluginMarket';
import { marketStatusLabel, permissionLabel, pluginFailure, type PluginFailure } from '../model/pluginPackages';
import PluginInstallDialog from './PluginInstallDialog.vue';

const INSTALLING = '\u0000install';
const RELEASING = '\u0000release';
const props = defineProps<{ installLink?: PluginInstallLink | null }>();
const emit = defineEmits<{ consumeInstallLink: [] }>();
const { locale } = useLocale();
const plugins = usePluginCatalog();
const market = usePluginMarket();
const { sources, selected, sourceOptions, catalog, loadingSources, loadingCatalog, saving, sourcesFailure, catalogFailure, loadSources, loadCatalog } = market;
const sourceErrorId = useId();
const draft = ref('');
const sourceError = ref('');
const query = ref('');
const busy = ref<string | null>(null);
const downloadingLinkedPackage = ref(false);
const feedback = shallowRef<{ failed: boolean; title: string; detail?: string } | null>(null);
const pending = shallowRef<MarketDownload | null>(null);
const installFailure = shallowRef<PluginFailure | null>(null);
let disposed = false;

onMounted(() => { void loadSources(); });
onBeforeUnmount(() => {
  disposed = true;
  // A download nobody confirmed is not kept.
  if (pending.value && busy.value !== INSTALLING) void release(pending.value.sha256);
});

const selectedSource = computed({
  get: () => selected.value ?? '',
  set: (value: string) => { selected.value = value; },
});

const key = (pluginId: string, version: string) => `${pluginId}@${version}`;

function entryName(entry: MarketPackage): string {
  return pluginText(entry.locales, locale.value, pluginTextKey.plugin(), entry.name);
}

function entryDescription(entry: MarketPackage): string | null {
  return entry.description ? pluginText(entry.locales, locale.value, pluginTextKey.description(), entry.description) : null;
}

const visible = computed(() => {
  const packages = catalog.value?.packages ?? [];
  const needle = query.value.trim().toLowerCase();
  if (!needle) return packages;
  return packages.filter(entry => [entry.id, entryName(entry), entryDescription(entry) ?? '', entry.publisher ?? '']
    .some(text => text.toLowerCase().includes(needle)));
});

function statusClass(status: MarketStatus): string {
  switch (status) {
    case 'installed': return 'success';
    case 'updateAvailable': return 'info';
    case 'installedNewer': return 'warn';
    case 'bundled':
    case 'unavailable':
    case 'notInstalled': return '';
  }
}

async function addSource() {
  const source = draft.value.trim();
  if (!source) return;
  sourceError.value = '';
  try {
    await market.addSource(source);
    if (!disposed) draft.value = '';
  } catch (cause) {
    if (!disposed) {
      const failure = pluginFailure(cause, tr("添加索引源失败"), tr);
      sourceError.value = `${failure.title}：${failure.detail}`;
    }
  }
}

async function pickDirectory() {
  try {
    const chosen = await open({ multiple: false, directory: true, title: tr("选择本地市场目录") });
    if (typeof chosen === 'string') draft.value = chosen;
  } catch (cause) {
    sourceError.value = tr("选择目录失败：{0}。请重试。", { 0: String(cause) });
  }
}

async function removeSelected() {
  const source = selected.value;
  if (!source) return;
  feedback.value = null;
  try {
    await market.removeSource(source);
  } catch (cause) {
    if (!disposed) feedback.value = { failed: true, ...pluginFailure(cause, tr("移除索引源失败"), tr) };
  }
}

/** Downloads one version and opens its inspection for confirmation. */
function download(entry: MarketPackage, version: string) {
  const source = selected.value;
  if (source && busy.value === null && !pending.value) {
    void inspectDownload(entry.id, version, source);
  }
}

watch(() => props.installLink, link => {
  if (!link) return;
  emit('consumeInstallLink');
  if (busy.value !== null || pending.value) {
    notifyError(tr("另一个插件安装正在进行"));
    return;
  }
  void inspectDownload(link.pluginId, link.version, null);
}, { immediate: true });

async function inspectDownload(pluginId: string, version: string, source: string | null) {
  busy.value = key(pluginId, version);
  downloadingLinkedPackage.value = source === null;
  feedback.value = null;
  try {
    const downloaded = source === null
      ? await pluginMarketDownloadOfficial(pluginId, version)
      : await pluginMarketDownload(source, pluginId, version);
    if (disposed) { await release(downloaded.sha256); return; }
    installFailure.value = null;
    pending.value = downloaded;
  } catch (cause) {
    if (!disposed) feedback.value = { failed: true, ...pluginFailure(cause, tr("下载失败"), tr) };
  } finally {
    downloadingLinkedPackage.value = false;
    busy.value = null;
  }
}

function release(sha256: string) {
  return pluginMarketRelease(sha256).catch(cause => { notifyError(tr("删除下载的插件包失败"), String(cause)); });
}

async function confirmInstall() {
  const downloaded = pending.value;
  if (!downloaded || busy.value !== null) return;
  busy.value = INSTALLING;
  installFailure.value = null;
  try {
    const installed = await plugins.install(downloaded.archive, downloaded.sha256);
    await release(downloaded.sha256);
    if (disposed) return;
    pending.value = null;
    feedback.value = {
      failed: false,
      title: installed.replaced
        ? tr("已更新 {0}：{1} → {2}", { 0: installed.pluginId, 1: installed.replaced, 2: installed.version })
        : tr("已安装 {0} {1}", { 0: installed.pluginId, 1: installed.version }),
    };
    await loadCatalog();
  } catch (cause) {
    if (disposed) await release(downloaded.sha256);
    else installFailure.value = pluginFailure(cause, tr("安装失败"), tr);
  } finally {
    busy.value = null;
  }
}

async function cancelInstall() {
  const downloaded = pending.value;
  pending.value = null;
  installFailure.value = null;
  if (downloaded) {
    busy.value = RELEASING;
    await release(downloaded.sha256);
    busy.value = null;
  }
}
</script>

<style scoped>
.market-panel { display: flex; flex-direction: column; gap: 12px; }
.source-form { display: flex; align-items: center; gap: 6px; }
.source-input { flex: 1; min-width: 0; }
.market-state { min-height: 160px; }
.alert-text { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 2px; overflow-wrap: anywhere; }
.alert-text span { font-size: 11px; }
.market-toolbar { display: flex; align-items: center; gap: 8px; }
.source-select { min-width: 0; max-width: 260px; }
.source-label { overflow: hidden; max-width: 260px; color: var(--text-secondary); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.market-search { flex: 1; min-width: 120px; }
.market-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; color: var(--text-secondary); font-size: 12px; }
.market-meta strong { color: var(--text-primary); font-size: 13px; font-weight: 600; }
.market-list { display: grid; gap: 12px; }
.market-card { padding: 12px 16px; }
.market-card-heading { display: flex; align-items: center; gap: 8px; }
.market-icon { display: grid; place-items: center; flex-shrink: 0; width: 32px; height: 32px; margin-right: 4px; border-radius: var(--radius-md); background: var(--primary-soft); color: var(--primary-color); }
.market-title { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 4px; }
.market-title strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 600; }
.market-title span { overflow: hidden; color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.market-description { margin: 8px 0 0 44px; color: var(--text-secondary); font-size: 12px; line-height: 1.5; overflow-wrap: anywhere; }
.market-attribution { display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 6px 0 0 44px; color: var(--text-secondary); font-size: 11px; }
.market-disclosure { margin-top: 8px; font-size: 11px; color: var(--text-secondary); }
.market-disclosure > summary { width: fit-content; min-height: 26px; border-radius: var(--radius-sm); }
.market-disclosure > summary:hover { color: var(--primary-color); }
.market-disclosure > summary:focus-visible { outline: 2px solid var(--primary-color); outline-offset: 2px; }
.version-list { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 8px 0 0; list-style: none; }
.version-list li { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; }
.version-number { min-width: 72px; color: var(--text-primary); font-size: 12px; }
.version-date, .version-size { color: var(--text-secondary); }
.version-install { margin-left: auto; }
@container settings (max-width: 520px) {
  .market-card-heading, .market-toolbar { flex-wrap: wrap; }
  .market-title { flex-basis: calc(100% - 40px); }
  .market-description, .market-attribution { margin-left: 0; }
}
</style>
