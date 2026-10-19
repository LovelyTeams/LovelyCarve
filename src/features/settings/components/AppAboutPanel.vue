<template>
  <section class="about-panel" :aria-label="tr('软件信息')">
    <div class="about-name"><BrandMark :size="24" /><strong>LovelyCarve</strong></div>
    <dl class="about-details">
      <div><dt>{{ tr("软件版本") }}</dt><dd class="mono">v{{ version }}</dd></div>
      <div><dt>{{ tr("构建时间（本地）") }}</dt><dd>{{ buildTime }}</dd></div>
    </dl>
    <div class="about-update" aria-live="polite">
      <button class="ui-btn sm" type="button" :disabled="isBusy" @click="handleUpdate">
        <Loader2 v-if="isBusy" :size="14" class="spin" aria-hidden="true" />
        <Download v-else-if="hasUpdate" :size="14" aria-hidden="true" />
        <RefreshCw v-else :size="14" aria-hidden="true" />
        {{ actionLabel }}
      </button>
      <span v-if="hasUpdate" class="ui-field-hint">{{ tr("发现新版本：v{0}", { 0: updateVersion }) }}</span>
      <span v-else-if="status === 'unsupported'" class="ui-field-hint">{{ tr("发现新版本：v{0}，暂不支持应用内安装", { 0: updateVersion }) }}</span>
      <span v-else-if="status === 'downloading' && downloadProgress !== null" class="ui-field-hint">{{ tr("正在下载更新：{0}%", { 0: downloadProgress }) }}</span>
      <span v-else-if="status === 'installing'" class="ui-field-hint">{{ tr("正在安装更新，即将重启…") }}</span>
      <span v-else-if="status === 'up-to-date'" class="ui-field-hint">{{ tr("已是最新版本") }}</span>
      <span v-else-if="status === 'error'" class="ui-field-hint danger" role="alert">{{ tr("更新失败：{0}", { 0: error }) }}</span>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { Download, Loader2, RefreshCw } from 'lucide-vue-next';
import BrandMark from '../../../components/display/BrandMark.vue';
import { useAppVersion } from '../../../composables/useAppVersion';
import { useAppUpdater } from '../composables/useAppUpdater';
import { formatEpochSeconds } from '../../../utils/format';

const { version, buildDate } = useAppVersion();
const { status, error, updateVersion, downloadProgress, isBusy, hasUpdate, checkForUpdates, installUpdate } = useAppUpdater();
const buildTime = computed(() => {
  const timestamp = Date.parse(buildDate);
  return Number.isFinite(timestamp) ? formatEpochSeconds(timestamp / 1000, { precision: 'seconds' }) : tr("未知");
});
const actionLabel = computed(() => {
  if (hasUpdate.value) return tr("更新");
  if (status.value === 'checking') return tr("正在检查更新…");
  if (status.value === 'downloading') return tr("正在下载…");
  if (status.value === 'installing') return tr("正在安装…");
  return tr("检查更新");
});

async function handleUpdate() {
  if (hasUpdate.value) await installUpdate();
  else await checkForUpdates();
}
</script>

<style scoped>
.about-panel { display: flex; flex-direction: column; gap: 24px; }
.about-name { display: flex; align-items: center; gap: 8px; font-size: 15px; }
.about-details { display: flex; flex-direction: column; gap: 16px; margin: 0; font-size: 12px; }
.about-details > div { display: flex; flex-wrap: wrap; gap: 8px 16px; }
.about-details dt { min-width: 112px; color: var(--text-secondary); }
.about-details dd { margin: 0; color: var(--text-primary); }
.about-update { display: flex; align-items: center; flex-wrap: wrap; gap: 8px 16px; }
.about-update .danger { color: var(--sev-critical-fg); }
</style>
