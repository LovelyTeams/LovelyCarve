<template>
  <div v-if="candidates === null" class="bde-mem-pick">
    <button class="ui-btn sm bde-file-btn" @click="emit('pick')">
      <FileSearch :size="13" />
      <span>{{ memoryPath ? tr("更换内存镜像") : tr("选择内存镜像") }}</span>
    </button>
    <div v-if="memoryPath" class="bde-file-path mono" :title="memoryPath">{{ memoryPath }}</div>
    <div class="bde-actions">
      <button class="ui-btn sm" @click="emit('close')">{{ tr("取消") }}</button>
      <button
        class="ui-btn sm primary"
        :disabled="!memoryPath"
        @click="emit('scan')"
      >{{ tr("开始扫描") }}</button>
    </div>
  </div>

  <template v-else>
    <div class="bde-mem-summary">
      {{ tr("找到") }} {{ candidates.length }} {{ tr("个 VMK 候选 ·") }}
      <span :class="{ 'bde-mem-derived': derivedCount > 0 }">{{ derivedCount }} {{ tr("个可派生恢复密钥") }}</span>
    </div>
    <div class="bde-mem-list">
      <button
        v-for="cand in candidates"
        :key="cand.offset + ':' + cand.vmk_hex"
        class="bde-mem-row mono"
        :class="{
          'is-derived': cand.recovery_key !== null,
          'is-tried': cand.already_tried,
        }"
        :disabled="cand.already_tried"
        :title="cand.already_tried ? tr('此 VMK 此前已尝试过') : tr('点击使用此 VMK 解锁')"
        @click="emit('unlock', cand)"
      >
        <span v-if="cand.recovery_key" class="bde-mem-key">{{ cand.recovery_key }}</span>
        <span v-else class="bde-mem-nokey">{{ tr("未能派生恢复密钥 · 点击仍可用此 VMK 解锁") }}</span>
        <span v-if="cand.already_tried" class="ui-badge sm bde-mem-state">{{ tr("已尝试") }}</span>
      </button>
    </div>
    <div class="bde-actions">
      <button class="ui-btn sm" @click="emit('reset')">{{ tr("重新扫描") }}</button>
      <button class="ui-btn sm" @click="emit('close')">{{ tr("关闭") }}</button>
    </div>
  </template>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { FileSearch } from 'lucide-vue-next';
import type { BitlockerVmkCandidate } from '../model/bitlocker';
const props = defineProps<{ memoryPath: string; candidates: BitlockerVmkCandidate[] | null }>();
const emit = defineEmits<{ pick: []; scan: []; reset: []; close: []; unlock: [candidate: BitlockerVmkCandidate] }>();
const derivedCount = computed(() => props.candidates?.filter(candidate => candidate.recovery_key !== null).length ?? 0);
</script>
<style scoped src="../styles/bitlocker-form.css"></style>
<style scoped>
.bde-mem-pick {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.bde-file-btn {
  align-self: flex-start;
  border-style: dashed;
}

.bde-file-path {
  font-size: 11px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.bde-mem-summary {
  font-size: 12px;
  color: var(--text-secondary);
}

.bde-mem-derived {
  color: var(--primary-color);
}

.bde-mem-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 260px;
  overflow-y: auto;
  padding-right: 2px;
}

.bde-mem-row {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  background: var(--surface-primary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  padding: 9px 11px;
  text-align: left;
  cursor: pointer;
  transition: border-color var(--transition-fast), background var(--transition-fast);
  font-size: 11px;
  color: var(--text-secondary);
}

.bde-mem-row:hover:not(:disabled) {
  border-color: var(--primary-color);
  background: var(--sidebar-active-bg);
}

.bde-mem-row.is-derived {
  border-color: color-mix(in srgb, var(--primary-color) 60%, var(--border-color));
}

.bde-mem-row.is-tried {
  opacity: 0.5;
  cursor: not-allowed;
}

.bde-mem-row:disabled {
  cursor: not-allowed;
}

/* The 48-digit recovery key is the actionable payload — show it prominently. */
.bde-mem-key {
  font-size: 13px;
  letter-spacing: 0.04em;
  color: var(--primary-color);
  word-break: break-all;
}

.bde-mem-nokey {
  font-family: var(--font-ui);
  font-size: 12px;
  color: var(--text-secondary);
}

</style>
