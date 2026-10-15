<template>
<section class="ui-card" data-section="integrity" aria-labelledby="integrity-title">
  <header class="ui-card-header">
    <h3 id="integrity-title" class="ui-card-title"><Fingerprint :size="15" />{{ tr("完整性校验") }}</h3>
    <span v-if="hashVerdict === 'match'" class="ui-badge success verdict-badge"><ShieldCheck :size="12" />{{ tr("与采集记录一致") }}</span>
    <span v-else-if="hashVerdict === 'mismatch'" class="ui-badge danger verdict-badge"><ShieldAlert :size="12" />{{ tr("与采集记录不一致") }}</span>
  </header>
  <div class="card-body">
    <ImageHashPanel :image-id="info.id" :size-bytes="info.size_bytes" />
    <div v-if="ewfHashRows.length > 0" class="stored-hashes">
      <h4 class="group-title">{{ tr("采集时记录的哈希") }}</h4>
      <dl class="stored-hash-list">
        <div v-for="row in ewfHashRows" :key="row.label" class="stored-hash-row" :class="`verdict-${row.verdict}`">
          <dt class="mono">{{ row.label }}</dt>
          <dd>
            <span class="stored-hash-value mono">{{ row.value }}</span>
            <span class="stored-hash-verdict">
              <template v-if="row.verdict === 'match'"><Check :size="12" />{{ tr("一致") }}
</template>
                      <template v-else-if="row.verdict === 'mismatch'"><X :size="12" />{{ tr("不一致") }}</template>
                      <template v-else>{{ tr("未校验") }}</template>
                    </span>
                    <button class="ui-icon-btn sm" :title="tr('复制采集时记录的 {0}', { 0: row.label })" :aria-label="tr('复制采集时记录的 {0}', { 0: row.label })" @click="emit('copy', tr('采集时记录的 {0}', { 0: row.label }), row.value)">
                      <Copy :size="12" />
                    </button>
                  </dd>
                </div>
              </dl>
            </div>
          </div>
        </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed } from 'vue';
import { Check, Copy, Fingerprint, ShieldAlert, ShieldCheck, X } from 'lucide-vue-next';
import type { ImageInfo } from '../../../lib/ipc/image';
import { evidenceHashVerdict, storedHashRows } from '../model/evidenceOverview';
import { useImageHashResult } from '../composables/useImageHash';
import ImageHashPanel from './ImageHashPanel.vue';
const props = defineProps<{ info: ImageInfo }>();
const emit = defineEmits<{ copy: [label: string, value: string] }>();
const hashResult = useImageHashResult(() => props.info.id);
const ewfHashRows = computed(() => storedHashRows(props.info, hashResult.value));
const hashVerdict = computed(() => evidenceHashVerdict(ewfHashRows.value));
</script>
<style scoped src="../styles/evidence-facts.css"></style>
