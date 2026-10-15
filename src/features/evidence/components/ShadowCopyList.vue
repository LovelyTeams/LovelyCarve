<template>
  <div class="shadow-list">
    <p v-if="volume.error" class="ui-alert danger" role="alert">{{ tr("VSS 元数据无法读取：") }}{{ volume.error }}</p>
    <ul v-else class="copy-list">
      <li v-for="copy in volume.shadow_copies" :key="copy.store_id" class="copy-row">
        <span class="copy-index mono">#{{ copy.index + 1 }}</span>
        <span class="copy-time mono" :title="localTime(copy)">{{ utcTime(copy) }}</span>
        <span class="copy-flags">
          <span v-if="copy.persistent" class="ui-badge sm">{{ tr("持久") }}</span>
          <span v-if="copy.client_accessible" class="ui-badge sm">{{ tr("客户端可访问") }}</span>
        </span>
        <span class="copy-machine" :title="copyTitle(copy)">{{ copy.operating_machine || copy.shadow_copy_id || copy.store_id }}</span>
        <button
          class="ui-btn sm"
          type="button"
          :disabled="opening !== null || copy.unavailable_reason !== null"
          :title="copy.unavailable_reason ?? (copy.open_image_id ? tr('切换到已打开的卷影副本') : tr('重建为只读镜像并在新标签页打开'))"
          @click="emit('open', copy)"
        >
          <Loader2 v-if="opening === shadowCopyKey(volume, copy)" :size="13" class="spin" />
          <ArrowUpRight v-else-if="copy.open_image_id" :size="13" />
          <FolderOpen v-else :size="13" />
          {{ opening === shadowCopyKey(volume, copy) ? tr("正在重建…") : copy.open_image_id ? tr("切换到") : tr("打开") }}
        </button>
      </li>
    </ul>
    <p v-if="error" class="ui-alert danger" role="alert">{{ tr("打开卷影副本失败：") }}{{ error }}</p>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { ArrowUpRight, FolderOpen, Loader2 } from 'lucide-vue-next';
import type { ShadowCopyInfo, ShadowVolumeInfo } from '../../../lib/ipc/shadow';
import { shadowCopyKey } from '../composables/useShadowCopies';
import { formatEpochSeconds } from '../../../utils/format';

// 镜像信息页分区行展开后的卷影副本列表：一个 NTFS 卷的副本（最早在前），
// 或该卷 VSS 元数据的读取错误。打开由页面统一调度，这里只发出所选副本。
defineProps<{
  volume: ShadowVolumeInfo;
  /** 正在重建的副本（`shadowCopyKey`）；有值时所有「打开」暂不可用。 */
  opening: string | null;
  /** 本卷上一次打开失败的原因。 */
  error: string | null;
}>();
const emit = defineEmits<{ open: [copy: ShadowCopyInfo] }>();

function seconds(copy: ShadowCopyInfo): number | null {
  return copy.created_unix_ms == null ? null : copy.created_unix_ms / 1000;
}

function utcTime(copy: ShadowCopyInfo): string {
  return formatEpochSeconds(seconds(copy), { timezone: 'utc', precision: 'seconds', suffix: true, fallback: tr("创建时间未知") });
}

function localTime(copy: ShadowCopyInfo): string {
  const local = formatEpochSeconds(seconds(copy), { precision: 'seconds', fallback: '' });
  return local ? tr("{0}（本地）", { 0: local }) : '';
}

function copyTitle(copy: ShadowCopyInfo): string {
  return [
    copy.operating_machine && tr("计算机：{0}", { 0: copy.operating_machine }),
    copy.shadow_copy_id && tr("卷影副本 ID：{0}", { 0: copy.shadow_copy_id }),
    copy.shadow_copy_set_id && tr("副本集 ID：{0}", { 0: copy.shadow_copy_set_id }),
    tr("存储区 ID：{0}", { 0: copy.store_id }),
  ].filter(Boolean).join('\n');
}
</script>

<style scoped>
.shadow-list { display: grid; gap: 8px; }
.copy-list { display: grid; margin: 0; padding: 0; list-style: none; }
.copy-row {
  display: grid;
  grid-template-columns: 36px minmax(170px, auto) auto minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  min-height: 36px;
  font-size: 12px;
}
.copy-row + .copy-row { border-top: 1px solid color-mix(in srgb, var(--border-color) 70%, transparent); }
.copy-index { color: var(--text-secondary); }
.copy-flags { display: flex; gap: 4px; }
.copy-machine { overflow: hidden; color: var(--text-secondary); text-overflow: ellipsis; white-space: nowrap; }
@container volume-card (max-width: 600px) {
  .copy-row { grid-template-columns: 36px minmax(0, 1fr) auto; }
  .copy-flags, .copy-machine { display: none; }
}
</style>
