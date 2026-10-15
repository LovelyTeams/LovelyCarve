<template>
    <Teleport to="body"><Transition name="ui-modal"><div v-if="open" class="ui-modal-overlay gs-modal-overlay" @keydown.esc.stop="emit('close')">
      <div :ref="elements.dialog" class="ui-modal gs-modal" role="dialog" aria-modal="true" :aria-label="tr('新建扫描')">
        <header class="ui-modal-header"><span class="ui-modal-icon"><ScanSearch :size="18" /></span><div class="ui-modal-heading"><h2 class="ui-modal-title">{{ tr("新建扫描") }}</h2><p class="ui-modal-subtitle">{{ tr("按文件签名从检材中识别并恢复文件") }}</p></div><button class="ui-icon-btn" :aria-label="tr('关闭扫描设置')" :disabled="starting" @click="emit('close')"><X :size="16" /></button></header>
        <div class="ui-modal-body gs-modal-body">        <div class="format-heading">
          <h3 class="ui-section-title">{{ tr("文件类型") }} <span class="ui-count">{{ selectedFormatIds.size }} / {{ formats.length }}</span></h3>
          <div class="format-actions"><button class="ui-btn link sm" type="button" @click="emit('select-all')">{{ tr("全选") }}</button><button class="ui-btn link sm" type="button" @click="emit('clear')">{{ tr("清空") }}</button></div>
        </div>
        <div v-if="formats.length" class="format-groups">
          <div v-for="group in formatGroups" :key="group.id" class="format-group">
            <label class="group-label">
              <input type="checkbox" :checked="group.formats.every(format => selectedFormatIds.has(format.id))"
                :indeterminate="group.formats.some(format => selectedFormatIds.has(format.id)) && !group.formats.every(format => selectedFormatIds.has(format.id))"
                @change="emit('toggle-group', group.id)" />
              <component :is="categoryIcon(group.id)" :size="16" /><strong>{{ tr(group.label) }}</strong>
            </label>
            <div class="format-chips">
              <label v-for="format in group.formats" :key="format.id" class="format-chip" :class="{ selected: selectedFormatIds.has(format.id) }" :title="format.description">
                <input type="checkbox" :checked="selectedFormatIds.has(format.id)" @change="emit('toggle-format', format.id)" />{{ format.label }}
              </label>
            </div>
          </div>
        </div>
        <p v-else class="scope-note">{{ initializing ? tr("正在加载文件类型…") : tr("文件类型加载失败，请重新打开此页。") }}</p>

<details v-disclosure-motion class="search-disclosure"><summary><span>{{ tr("扫描范围") }}</span><small>{{ scopeOptions.find(option => option.value === scope)?.label }}</small></summary><div class="disclosure-body">
        <div class="ui-seg gs-scope-tabs" role="group" :aria-label="tr('扫描范围')">
          <button v-for="option in scopeOptions" :key="option.value" class="ui-seg-item" type="button"
            :aria-pressed="scope === option.value" :disabled="option.value !== 'whole' && partitions.length === 0"
            :class="{ active: scope === option.value }" @click="scope = option.value">
            <component :is="option.icon" :size="15" /><span>{{ option.label }}</span>
          </button>
        </div>
        <label v-if="scope === 'partition'" class="ui-field partition-field">
          <span class="ui-field-label">{{ tr("目标分区") }}</span>
          <UiSelect v-model="partitionId" :options="partitionOptions" />
        </label>
        <p class="scope-note"><Info :size="14" />{{ scope === 'unallocated' ? tr("仅扫描分区之间的空隙，不含文件系统内部的空闲空间。") : scope === 'partition' ? tr("只扫描所选分区。") : tr("扫描整个镜像，覆盖所有分区。") }}</p>

</div></details>        <details v-disclosure-motion class="search-disclosure">
          <summary><span>{{ tr("高级选项") }}</span><small>{{ tr("单文件") }} {{ formatBytes(maxFileSize) }} {{ tr("· 最多") }} {{ maxResults.toLocaleString() }} {{ tr("个结果") }}</small></summary>
          <div class="disclosure-body advanced-fields">
            <label class="ui-field"><span class="ui-field-label">{{ tr("单文件大小上限") }}</span><UiSelect v-model="maxFileSize" :options="MAX_FILE_SIZE_OPTIONS" /></label>
            <label class="ui-field"><span class="ui-field-label">{{ tr("结果数量上限") }}</span><UiSelect v-model="maxResults" :options="MAX_RESULTS_OPTIONS" /></label>
          </div>
        </details>
</div>
        <footer class="ui-modal-footer gs-modal-footer"><button class="ui-btn" :disabled="starting" @click="emit('close')">{{ tr("取消") }}</button><button class="ui-btn primary" :disabled="!canStart || starting" @click="emit('start')"><Loader2 v-if="starting" :size="14" class="spin" /><Play v-else :size="14" />{{ starting ? tr("正在创建…") : tr("开始扫描") }}</button></footer>
      </div>
    </div></Transition></Teleport>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import type { Ref } from 'vue';
import { ScanSearch, X, Info, Loader2, Play } from 'lucide-vue-next';
import UiSelect from '../../../components/ui/UiSelect.vue';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import type { CarveCategory, CarveFormatInfo, CarveScope } from '../../../lib/ipc/carve';
import type { DiskPartition } from '../../../lib/ipc/image';
import { formatBytes } from '../../../utils/format';
import { scopeOptions, categoryIcon } from './carveIcons';
import { MAX_FILE_SIZE_OPTIONS, MAX_RESULTS_OPTIONS } from '../model/carveView';
const scope = defineModel<CarveScope>('scope', { required: true });
const partitionId = defineModel<number | null>('partitionId', { required: true });
const maxFileSize = defineModel<number>('maxFileSize', { required: true });
const maxResults = defineModel<number>('maxResults', { required: true });
defineProps<{ open: boolean; formats: CarveFormatInfo[]; formatGroups: { id: CarveCategory; label: string; formats: CarveFormatInfo[] }[];
  selectedFormatIds: ReadonlySet<string>; initializing: boolean; starting: boolean; canStart: boolean;
  partitions: DiskPartition[]; partitionOptions: { value: number; label: string }[]; elements: { dialog: Ref<HTMLElement | null> } }>();
const emit = defineEmits<{ close: []; start: []; 'select-all': []; clear: []; 'toggle-group': [category: CarveCategory]; 'toggle-format': [id: string] }>();
</script>
<style scoped src="../../../styles/analysis-workspace.css"></style>
<style scoped src="../styles/carve-workspace.css"></style>
