<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { ChevronLeft, ChevronRight, CircleAlert, Crosshair } from 'lucide-vue-next';
import { fsReadRange } from '../../../../lib/ipc/fs/read';
import PreviewChromeSlot from '../PreviewChromeSlot.vue';
import PreviewState from '../PreviewState.vue';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';

const props = defineProps<{ imageId: ImageId; nodeId: NodeId; size: number; target: { offset: number; length: number } }>();
const WINDOW_SIZE = 4096;
const start = ref(0);
const bytes = ref(new Uint8Array());
const loading = ref(false);
const error = ref('');
const content = ref<HTMLElement | null>(null);
let generation = 0;
const end = computed(() => start.value + bytes.value.length);
const rows = computed(() => {
  const result = [];
  for (let index = 0; index < bytes.value.length; index += 16) {
    const cells = Array.from(bytes.value.subarray(index, index + 16), (byte, column) => {
      const offset = start.value + index + column;
      return { offset, hex: byte.toString(16).toUpperCase().padStart(2, '0'),
        char: byte >= 32 && byte <= 126 ? String.fromCharCode(byte) : '.',
        hit: offset >= props.target.offset && offset - props.target.offset < props.target.length };
    });
    result.push({ offset: start.value + index, cells });
  }
  return result;
});
const hex = (value: number) => value.toString(16).toUpperCase().padStart(8, '0');

async function load() {
  const request = ++generation;
  loading.value = true;
  error.value = '';
  bytes.value = new Uint8Array();
  try {
    if (!Number.isSafeInteger(props.target.offset) || props.target.offset < 0 || props.target.offset >= props.size) {
      throw new Error(tr("命中偏移超出文件范围，文件内容可能已变化"));
    }
    const result = await fsReadRange({ imageId: props.imageId, nodeId: props.nodeId, offset: start.value, length: Math.min(WINDOW_SIZE, props.size - start.value) });
    if (request !== generation) return;
    bytes.value = new Uint8Array(result);
    if (!bytes.value.length) throw new Error(tr("该位置没有可读取的内容"));
    if (props.target.offset >= start.value && props.target.offset < start.value + WINDOW_SIZE && props.target.offset >= end.value) {
      throw new Error(tr("读取内容未覆盖命中位置，文件可能已变化或无法完整读取"));
    }
  } catch (reason) {
    if (request === generation) error.value = String(reason);
  } finally {
    if (request === generation) {
      loading.value = false;
      await nextTick();
      if (request === generation) content.value?.querySelector('.hit')?.scrollIntoView({ block: 'center', inline: 'nearest' });
    }
  }
}
function goTo(offset: number) {
  start.value = Math.floor(offset / WINDOW_SIZE) * WINDOW_SIZE;
  void load();
}
watch(() => [props.imageId, props.nodeId, props.size, props.target.offset, props.target.length], () => goTo(props.target.offset), { immediate: true });
onBeforeUnmount(() => { generation++; });
</script>

<template>
  <section class="range-preview" :aria-label="tr('命中字节预览')">
    <!-- 回到命中放进视图栏；命中位置与分段翻页放进状态栏。 -->
    <PreviewChromeSlot area="tools">
      <button type="button" class="ui-btn sm ghost" :title="tr('回到命中位置')" :aria-label="tr('回到命中位置')" :disabled="loading" @click="goTo(target.offset)">
        <Crosshair :size="13" />{{ tr("回到命中") }}
      </button>
    </PreviewChromeSlot>
    <PreviewChromeSlot area="statusStart">
      <span class="ui-footbar-stat">{{ tr("命中") }} <strong>0x{{ hex(target.offset) }}</strong> {{ target.length }} {{ tr("字节") }}</span>
    </PreviewChromeSlot>
    <PreviewChromeSlot area="statusEnd">
      <span class="ui-footbar-stat mono">{{ start.toLocaleString() }}–{{ end.toLocaleString() }} / {{ size.toLocaleString() }} B</span>
      <span class="range-actions">
        <button type="button" class="ui-icon-btn sm" :title="tr('上一段')" :aria-label="tr('上一段')" :disabled="loading || start <= 0" @click="goTo(start - WINDOW_SIZE)"><ChevronLeft :size="14" /></button>
        <button type="button" class="ui-icon-btn sm" :title="tr('下一段')" :aria-label="tr('下一段')" :disabled="loading || start + WINDOW_SIZE >= size" @click="goTo(start + WINDOW_SIZE)"><ChevronRight :size="14" /></button>
      </span>
    </PreviewChromeSlot>
    <PreviewState v-if="loading" tone="loading" :title="tr('正在读取命中位置…')" />
    <PreviewState v-else-if="error" tone="error" :icon="CircleAlert" :title="tr('无法读取命中位置')" :description="error">
      <button type="button" class="ui-btn sm" @click="load">{{ tr("重试") }}</button>
    </PreviewState>
    <div v-else ref="content" class="range-content mono" tabindex="0" :aria-label="tr('十六进制内容')">
      <div class="hex-row heading"><span>Offset</span><span>HEX</span><span>ASCII</span></div>
      <div v-for="row in rows" :key="row.offset" class="hex-row">
        <span class="offset">{{ hex(row.offset) }}</span>
        <span class="byte-cells"><span v-for="cell in row.cells" :key="cell.offset" :class="{ hit: cell.hit }" :data-offset="cell.offset">{{ cell.hex }}</span></span>
        <span class="ascii"><span v-for="cell in row.cells" :key="cell.offset" :class="{ hit: cell.hit }">{{ cell.char }}</span></span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.range-preview { display: flex; flex-direction: column; min-height: 0; height: 100%; }
.range-actions { display: inline-flex; gap: 4px; }
.range-content { flex: 1; min-height: 0; overflow: auto; padding: 0 16px 12px; font-size: 12px; line-height: 24px; outline: none; }
.hex-row { display: grid; grid-template-columns: 10ch 47ch 16ch; column-gap: 2ch; width: max-content; }
.heading, .offset { color: var(--text-secondary); }
.heading { position: sticky; top: 0; padding-top: 8px; border-bottom: 1px solid var(--border-color); background: var(--surface-secondary); }
.byte-cells { display: grid; grid-template-columns: repeat(16, 2ch); gap: 1ch; }
.hit { color: var(--primary-color); background: var(--primary-soft); outline: 1px solid var(--primary-color); border-radius: var(--radius-sm); }
</style>
