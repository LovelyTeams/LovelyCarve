<template>
<div class="mount-wrap" @click.stop>
  <button
    class="ui-btn sm mount-btn"
    :class="{ active: view }"
    :disabled="busy || pickerLoading"
    @click="emit('toggle')"
    :title="view ? tr('已挂载到 {0}，点击卸载', { 0: view!.mount_point }) : (mountTarget === 'folder' ? tr('挂载到只读文件夹') : tr('挂载为只读盘符'))"
  >
    <Loader2 v-if="busy || (pickerLoading && pickerOpen)" :size="13" class="spin" />
    <HardDriveUpload v-else-if="view" :size="13" />
    <HardDriveDownload v-else :size="13" />
    <span class="mount-btn-text mono">
      {{ view ? mountLabel(view!) : tr("挂载") }}
    </span>
  </button>
  <button
    v-if="view"
    class="ui-btn sm share-btn"
    :class="{ shared: view!.share }"
    :disabled="shareBusy"
    @click="emit('share')"
    :title="view!.share ? tr('已共享为 {0}，点击取消共享', { 0: view!.share!.unc_path }) : tr('把该只读挂载发布为局域网共享')"
  >
    <Loader2 v-if="shareBusy" :size="13" class="spin" />
    <Share2 v-else :size="13" />
    <span class="mount-btn-text mono">{{ view!.share ? tr("共享中") : tr("共享") }}</span>
  </button>
  <button
    v-if="view?.share"
    class="ui-icon-btn sm bordered"
    :title="tr('复制共享名')"
    :aria-label="tr('复制共享名')"
    @click="emit('copy', view!.share!.unc_path)"
  >
    <Copy :size="12" />
  </button>
  <div v-if="pickerOpen && availableDrives.length > 0" class="ui-menu drive-picker">
    <div class="ui-menu-label">{{ tr("选择盘符") }}</div>
    <div class="drive-grid">
      <button
        v-for="d in availableDrives"
        :key="d"
        class="ui-btn sm mono drive-letter"
        @click="emit('mount', d)"
      >{{ d.replace(/\\$/, '') }}</button>
    </div>
  </div>
</div>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { Copy, HardDriveDownload, HardDriveUpload, Loader2, Share2 } from 'lucide-vue-next';
import type { DokanMountView } from '../../../lib/ipc/mount';
import type { MountTarget } from '../../../composables/useUiPreferences';
import { mountLabel } from '../model/mounts';
defineProps<{ view: DokanMountView | null; busy: boolean; shareBusy: boolean; mountTarget: MountTarget; pickerLoading: boolean; pickerOpen: boolean; availableDrives: string[] }>();
const emit = defineEmits<{ toggle: []; share: []; mount: [drive: string]; copy: [value: string] }>();
</script>
<style scoped>
.mount-wrap { position: relative; display: inline-flex; align-items: center; gap: 6px; }
.share-btn.shared { border-color: var(--sev-success-border); background: var(--sev-success-bg); color: var(--sev-success-fg); }
.mount-btn-text { font-size: 11px; letter-spacing: 0.02em; }
.drive-picker { position: absolute; top: calc(100% + 6px); right: 0; z-index: 20; min-width: 200px; padding: 4px 8px 8px; }
.drive-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 4px; }
.drive-letter:hover:not(:disabled) { border-color: var(--primary-solid); background: var(--primary-solid); color: var(--accent-contrast); }

</style>
