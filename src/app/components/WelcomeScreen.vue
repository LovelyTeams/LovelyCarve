<template>
  <section class="start-page" :aria-label="tr('起始页')">
    <div class="start-content">
      <header class="start-brand">
        <BrandMark :size="96" variant="background" />
        <div>
          <h1>LovelyCarve<span>.</span></h1>
          <p>{{ tr("磁盘取证 / 数据恢复工具") }}</p>
        </div>
      </header>
      <div class="start-cards">
        <section class="ui-card import-card" :class="{ 'is-dragging': dragging }" aria-labelledby="import-title" :aria-busy="busy">
          <div class="import-art" aria-hidden="true">
            <svg class="art-disk" viewBox="0 0 240 128" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" focusable="false">
              <ellipse class="art-shadow" cx="120" cy="118" rx="76" ry="6" stroke="none" />
              <path class="art-guides" d="M36 48V36h12M192 92v12h-12" />
              <g transform="rotate(-8 110 66)">
                <rect class="art-housing" x="56" y="18" width="108" height="96" rx="12" />
                <circle class="art-platter" cx="102" cy="58" r="28" />
                <circle class="art-track" cx="102" cy="58" r="20" />
                <path d="M82 58a20 20 0 0 1 20-20" />
                <circle class="art-hub" cx="102" cy="58" r="6" />
                <path class="art-arm" d="m142 84-18-36-6 6 18 32" />
                <circle class="art-hub" cx="140" cy="86" r="6" />
                <path class="art-detail" d="M70 98h32m8 0h12M68 30h2m80 72h2" />
              </g>
              <path class="art-transfer" d="M184 82v8a10 10 0 0 1-10 10h-8" />
            </svg>
            <FileDown :size="64" :stroke-width="0.75" class="art-image" />
          </div>
          <h2 id="import-title">{{ dragging ? tr("松开以添加证据") : tr("拖入文件") }}</h2>
          <p class="format-label">{{ tr("支持镜像与归档格式") }}</p>
          <div class="format-list" :aria-label="tr('常用支持格式')">
            <span v-for="format in formats" :key="format" class="ui-badge mono outline">{{ format }}</span>
          </div>
          <div class="import-actions">
            <button class="ui-btn lg primary block" type="button" :title="tr('添加证据 (Ctrl+O)')" :disabled="busy" @click="$emit('open')">
              <Loader2 v-if="busy" :size="18" class="spin" aria-hidden="true" />
              <FolderOpen v-else :size="18" aria-hidden="true" />
              <span>{{ busy ? loadLabel : tr("选择文件") }}</span>
              <ArrowRight v-if="!busy" :size="18" aria-hidden="true" />
            </button>
            <button class="ui-btn lg block" type="button" :disabled="busy" @click="$emit('open-case')">
              <BriefcaseBusiness :size="18" aria-hidden="true" /><span>{{ tr("案件管理") }}</span>
            </button>
          </div>
          <p class="shortcut"><kbd class="ui-kbd">Ctrl</kbd><kbd class="ui-kbd">O</kbd><span>{{ tr("添加镜像、文件夹或磁盘") }}</span></p>
          <span class="sr-only" role="status">{{ busy ? loadLabel : '' }}</span>
        </section>
        <section class="ui-card recent-card" aria-labelledby="recent-title">
          <header class="recent-header">
            <h2 id="recent-title" ref="recentTitle" tabindex="-1">{{ tr("最近打开的案件") }}</h2>
            <button ref="clearRecentButton" type="button" class="ui-btn ghost sm" :title="tr('清空最近案件记录')" :disabled="busy || clearingRecent || !recentCases.length" @click="handleClearRecent">{{ tr("清空") }}</button>
          </header>
          <div v-if="recentCases.length" class="recent-list ui-stagger">
            <button v-for="entry in recentCases" :key="entry.dir" class="recent-entry" type="button" :disabled="busy" :title="entry.dir" @click="$emit('open-recent', entry)">
              <span class="entry-icon"><BriefcaseBusiness :size="20" aria-hidden="true" /></span>
              <span class="entry-text">
                <span class="entry-name">{{ entry.name }}</span>
                <time :datetime="entry.openedAt">{{ formatDate(entry.openedAt) }}</time>
              </span>
              <ChevronRight :size="16" class="entry-arrow" aria-hidden="true" />
            </button>
          </div>
          <div v-else class="ui-empty recent-empty">
            <span class="ui-empty-icon" aria-hidden="true"><History :size="26" :stroke-width="1.5" /></span>
            <p class="ui-empty-title">{{ tr("还没有最近打开的案件") }}</p>
            <p class="ui-empty-desc">{{ tr("创建或打开案件后，会显示在这里") }}</p>
          </div>
        </section>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../composables/useLocale';

import { nextTick, ref } from 'vue';
import { ArrowRight, BriefcaseBusiness, ChevronRight, FileDown, FolderOpen, History, Loader2 } from 'lucide-vue-next';
import BrandMark from '../../components/display/BrandMark.vue';
import { confirm } from '../../composables/useConfirm';
import type { RecentCase } from '../../features/cases/api';
import { formatEpochSeconds } from '../../utils/format';

const props = defineProps<{ busy: boolean; dragging: boolean; loadLabel: string; recentCases: RecentCase[] }>();
const emit = defineEmits<{ open: []; 'open-case': []; 'open-recent': [entry: RecentCase]; 'clear-recent': [] }>();
const clearingRecent = ref(false);
const clearRecentButton = ref<HTMLButtonElement | null>(null);
const recentTitle = ref<HTMLHeadingElement | null>(null);

const formats = ['.dd', '.img', '.raw', '.E01', '.AD1', '.vmdk', '.vhdx', '.qcow2', '.vdi', '.iso', '.zip', '.7z', '.tar'];
function formatDate(value: string) {
  const timestamp = formatEpochSeconds(Date.parse(value) / 1000, { precision: 'seconds', fallback: '', zeroAsMissing: false });
  return timestamp ? tr("{0}（本地）", { 0: timestamp }) : tr("时间未知");
}

async function handleClearRecent() {
  if (props.busy || clearingRecent.value || !props.recentCases.length) return;
  clearingRecent.value = true;
  try {
    const accepted = await confirm({
      title: tr("清空最近案件记录"),
      message: tr("将清除本地最近案件记录。当前案件、案件目录和案件登记均会保留；创建或打开案件后会重新记录。"),
      confirmText: tr("清空记录"),
      danger: true,
    });
    if (accepted && !props.busy) emit('clear-recent');
  } finally {
    clearingRecent.value = false;
    await nextTick();
    if (clearRecentButton.value && !clearRecentButton.value.disabled) clearRecentButton.value.focus();
    else recentTitle.value?.focus();
  }
}
</script>

<style scoped>
.start-page {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  container: start-page / inline-size;
  padding: 48px 48px 56px;
  background: radial-gradient(ellipse at 88% 2%, color-mix(in srgb, var(--brand-glow-amber) 28%, transparent), transparent 42%), var(--bg-primary);
}
.start-content { width: min(1120px, 100%); margin: auto; }
.start-brand { display: flex; align-items: center; gap: 20px; margin-bottom: 32px; }
.start-brand h1 { margin: 0; font: 700 44px/1.2 var(--font-brand), sans-serif; letter-spacing: -0.02em; color: var(--text-primary); }
.start-brand h1 span { color: var(--brand-dot-color); }
.start-brand p { margin: 12px 0 0; color: var(--text-secondary); font-size: 13px; letter-spacing: 0.06em; }
.start-cards { display: grid; grid-template-columns: 1.12fr 1fr; gap: 24px; }
.import-card { display: flex; flex-direction: column; align-items: center; min-height: 444px; padding: 24px; border-color: color-mix(in srgb, var(--primary-color) 25%, var(--border-color)); background: radial-gradient(ellipse at 50% 18%, var(--surface-secondary), color-mix(in srgb, var(--primary-color) 5%, var(--surface-secondary))); transition: border-color var(--transition-fast), box-shadow var(--transition-fast); }
.import-card > * { flex-shrink: 0; }
.import-card.is-dragging { border-color: var(--primary-color); box-shadow: var(--card-shadow-hover); }
.import-art { position: relative; flex-shrink: 0; width: 240px; height: 128px; color: var(--primary-color); }
.art-disk { display: block; width: 100%; height: 100%; }
.art-shadow { fill: color-mix(in srgb, var(--primary-color) 8%, transparent); }
.art-guides { stroke: color-mix(in srgb, var(--primary-color) 30%, var(--surface-secondary)); }
.art-housing { fill: color-mix(in srgb, var(--primary-color) 8%, var(--surface-secondary)); }
.art-platter { fill: var(--surface-secondary); }
.art-track { stroke: color-mix(in srgb, var(--primary-color) 25%, var(--surface-secondary)); }
.art-hub { fill: color-mix(in srgb, var(--primary-color) 16%, var(--surface-secondary)); }
.art-arm { fill: var(--surface-secondary); }
.art-detail { stroke: var(--text-secondary); }
.art-transfer { stroke-dasharray: 3 5; }
.art-image { position: absolute; left: 144px; top: 8px; fill: var(--surface-secondary); transition: translate var(--motion-normal) var(--ease-out); }
.import-card.is-dragging .art-image { translate: 0 4px; }
h2 { font-family: var(--font-ui), sans-serif; color: var(--text-primary); }
.import-card h2 { margin: 12px 0; font-size: 16px; font-weight: 600; }
.format-label { color: var(--text-secondary); font-size: 12px; margin: 0 0 8px; }
.format-list { display: flex; justify-content: center; flex-wrap: wrap; gap: 6px; max-width: 360px; }
.format-list .ui-badge { background: var(--surface-secondary); }
.import-actions { display: flex; flex-direction: column; gap: 8px; width: min(280px, 100%); margin-top: 24px; }
.import-actions .ui-btn.primary { font-size: 14px; }
.import-actions .ui-btn:not(.primary) { color: var(--text-secondary); }
.import-actions .ui-btn:not(.primary):hover:not(:disabled) { color: var(--text-primary); }
.shortcut { display: flex; flex-wrap: wrap; justify-content: center; align-items: center; gap: 4px; color: var(--text-secondary); font-size: 11px; margin: 16px 0 0; }
.shortcut span { margin-left: 6px; }
.recent-card { height: 444px; padding: 24px; display: flex; flex-direction: column; }
.recent-header { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 16px; }
.recent-header h2 { font-size: 16px; font-weight: 600; margin: 0; }
.recent-list { overflow-y: auto; min-height: 0; flex: 1; padding-right: 4px; }
.recent-entry { display: flex; align-items: center; gap: 12px; width: 100%; text-align: left; padding: 12px 8px; border: none; border-top: 1px solid var(--border-color); border-radius: var(--radius-md); background: transparent; color: inherit; cursor: pointer; transition: background var(--transition-fast); }
.recent-entry:first-child { border-top-color: transparent; }
.recent-entry:hover:not(:disabled) { background: var(--hover-bg); }
.recent-entry:hover:not(:disabled), .recent-entry:hover + .recent-entry { border-top-color: transparent; }
.recent-entry:disabled { opacity: 0.5; cursor: default; }
.recent-entry:focus-visible { outline: 2px solid var(--primary-color); outline-offset: -2px; }
.entry-icon { width: 40px; height: 40px; border-radius: var(--radius-lg); background: var(--sev-info-bg); color: var(--primary-color); display: grid; place-items: center; flex-shrink: 0; }
.entry-text { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 4px; }
.entry-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-primary); font-size: 13px; font-weight: 500; }
.entry-text time { color: var(--text-secondary); font-family: var(--font-mono); font-size: 11px; line-height: 1.5; overflow-wrap: anywhere; }
.entry-arrow { color: var(--text-secondary); opacity: 0; flex-shrink: 0; transition: opacity var(--transition-fast), transform var(--transition-fast); }
.recent-entry:hover .entry-arrow, .recent-entry:focus-visible .entry-arrow { opacity: 1; transform: translateX(2px); }
.recent-empty { flex: 1; padding-bottom: 32px; }
/* Entrance: the mark pops, the wordmark and cards rise in turn, recent cases
   cascade; then the import art idles. Motion off leaves the page static. */
:root[data-motion='full'] .start-brand :deep(.brand-mark) { animation: ui-bounce-in var(--motion-slow) var(--ease-spring) backwards; }
:root[data-motion='full'] .start-brand > div { animation: ui-rise-in var(--motion-slow) var(--ease-out) 80ms backwards; }
:root[data-motion='full'] .import-card,
:root[data-motion='full'] .recent-card { --ui-rise-distance: 16px; animation: ui-rise-in var(--motion-slow) var(--ease-out) 140ms backwards; }
:root[data-motion='full'] .recent-card { animation-delay: 220ms; }
:root[data-motion='full'] .recent-list { --ui-stagger-base: 320ms; --ui-rise-distance: 8px; }
:root[data-motion='full'] .art-image { animation: ui-float 2.6s ease-in-out 600ms infinite; }
:root[data-motion='full'] .import-card.is-dragging .art-image { animation: none; }
@media (max-width: 800px) {
  .start-page { padding: 24px 20px; }
}
@container start-page (max-width: 880px) {
  .start-cards { grid-template-columns: 1fr; gap: 20px; }
  .start-brand { gap: 12px; margin-bottom: 24px; }
  .start-brand :deep(.brand-mark) { width: 72px; height: 72px; }
  .start-brand h1 { font-size: 32px; }
  .recent-card { height: 340px; }
}
@media (max-height: 760px) and (min-width: 801px) {
  .start-page { padding-top: 24px; padding-bottom: 24px; }
  .start-brand { margin-bottom: 20px; }
  .start-brand :deep(.brand-mark) { width: 76px; height: 76px; }
}
</style>
