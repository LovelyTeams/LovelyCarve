<template>
  <section class="about-panel" :aria-label="tr('软件信息')">
    <div class="about-name"><BrandMark :size="24" /><strong>LovelyCarve</strong></div>
    <dl class="about-details">
      <div><dt>{{ tr("软件版本") }}</dt><dd class="mono">v{{ version }}</dd></div>
      <div><dt>{{ tr("构建时间（本地）") }}</dt><dd>{{ buildTime }}</dd></div>
    </dl>
    <div class="update-section">
      <label class="update-preference">
        <span><strong>{{ tr('自动检查更新') }}</strong><small>{{ tr('默认关闭；开启后每次启动检查 GitHub tags。') }}</small></span>
        <input v-model="automatic" class="ui-switch" type="checkbox" role="switch" :aria-label="tr('自动检查更新')" />
      </label>
      <p class="ui-field-hint">{{ tr('仅检查版本，不会自动下载或安装；正式版忽略预发布标签。') }}</p>
      <div class="update-actions">
        <button class="ui-btn" type="button" :disabled="status === 'checking'" @click="checkNow">
          <Loader2 v-if="status === 'checking'" :size="14" class="spin" aria-hidden="true" />
          <RefreshCw v-else :size="14" aria-hidden="true" />
          {{ status === 'checking' ? tr('正在检查更新…') : tr('检查更新') }}
        </button>
        <button class="ui-btn ghost" type="button" :title="tagUrl" @click="copyTagUrl"><Check v-if="copied" :size="14" aria-hidden="true" /><Copy v-else :size="14" aria-hidden="true" />{{ copied ? tr('已复制') : tr('复制 GitHub 标签链接') }}</button>
      </div>
      <div aria-live="polite" role="status">
        <p v-if="status === 'available'" class="ui-alert info">{{ tr('发现新版本 {0}', { 0: latestTag ?? '' }) }}</p>
        <p v-else-if="status === 'current'" class="ui-alert success">{{ tr('未发现更新的版本标签。') }}</p>
        <p v-else-if="status === 'error'" class="ui-alert danger">{{ errorMessage }}</p>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, ref, watch } from 'vue';
import { Check, Copy, Loader2, RefreshCw } from 'lucide-vue-next';
import BrandMark from '../../../components/display/BrandMark.vue';
import type { AppUpdateCheck } from '../composables/useAppUpdateCheck';
import { formatEpochSeconds } from '../../../utils/format';
import { notifyError } from '../../../composables/useToast';

const props = defineProps<{ updates: AppUpdateCheck }>();
const { version, buildDate, automatic, status, latestTag, tagUrl, errorMessage, checkNow } = props.updates;
const copied = ref(false);
watch(tagUrl, () => { copied.value = false; });
async function copyTagUrl() {
  try {
    await navigator.clipboard.writeText(tagUrl.value);
    copied.value = true;
  } catch (cause) {
    notifyError(tr('复制失败'), String(cause));
  }
}
const buildTime = computed(() => {
  const timestamp = Date.parse(buildDate);
  return Number.isFinite(timestamp) ? formatEpochSeconds(timestamp / 1000, { precision: 'seconds' }) : tr("未知");
});
</script>

<style scoped>
.about-panel { display: flex; flex-direction: column; gap: 24px; }
.about-name { display: flex; align-items: center; gap: 8px; font-size: 15px; }
.about-details { display: flex; flex-direction: column; gap: 16px; margin: 0; font-size: 12px; }
.about-details > div { display: flex; flex-wrap: wrap; gap: 8px 16px; }
.about-details dt { min-width: 112px; color: var(--text-secondary); }
.about-details dd { margin: 0; color: var(--text-primary); }
.update-section { display: flex; flex-direction: column; gap: 12px; padding-top: 20px; border-top: 1px solid var(--border-color); }
.update-preference { display: flex; align-items: center; justify-content: space-between; gap: 16px; cursor: pointer; }
.update-preference span { display: flex; flex-direction: column; gap: 8px; }
.update-preference strong { font-size: 13px; font-weight: 600; }
.update-preference small { color: var(--text-secondary); font-size: 11px; line-height: 1.5; }
.update-section p { margin: 0; }
.update-actions { display: flex; flex-wrap: wrap; gap: 8px; }
</style>
