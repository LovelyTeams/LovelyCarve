<template>
  <div class="recovery-browser">
    <UiTransition kind="horizontal"><RecoveryTreeRail v-show="treeVisible" :rows="treeRows" :current-key="currentKey" :expanded="expanded" :loading="loading"
      @navigate="emit('navigate', $event)" @toggle="emit('toggle-expanded', $event)" /></UiTransition>
    <div class="browser-main">
      <!-- 地址栏：与文件列表一致，目录树开关、返回上一级与面包屑。 -->
      <div class="path-bar">
        <button class="ui-icon-btn" type="button" :aria-pressed="treeVisible" :aria-label="tr('切换目录树')" :title="tr('显示 / 隐藏目录树')" @click="treeVisible = !treeVisible"><PanelLeft :size="16" /></button>
        <button class="ui-icon-btn" type="button" :aria-label="tr('返回上一级')" :title="tr('返回上一级')" :disabled="!breadcrumbs.length" @click="emit('go-up')"><ArrowUp :size="16" /></button>
        <nav class="breadcrumb mono" :aria-label="tr('当前恢复目录')">
          <button class="crumb root" type="button" :class="{ active: !breadcrumbs.length }" :title="tr('全部恢复结果')" @click="emit('navigate', RECOVERY_ROOT)"><HardDrive :size="13" class="crumb-root-icon" /><span>{{ tr("恢复结果") }}</span></button>
          <template v-for="(item, index) in breadcrumbs" :key="item.key">
            <ChevronRight :size="12" class="crumb-sep" aria-hidden="true" />
            <button class="crumb" type="button" :class="{ active: index === breadcrumbs.length - 1, deleted: item.node?.deleted }" :title="item.name" @click="emit('navigate', item.key)">{{ item.name }}</button>
          </template>
          <Layers3 v-if="filtered" :size="12" class="crumb-flat-icon" :aria-label="tr('筛选结果包含子目录中的文件')" />
        </nav>
      </div>
      <RecoveryFileTable :rows="rows" :signature-columns="signatureColumns" :selected-key="selectedKey" :context-key="tableContext"
        :is-checked="isChecked" :is-partial="isPartial" :all-checked="allChecked" :some-checked="someChecked" :busy="busy" :loading="loading"
        :filtered="filtered" :empty-text="emptyText" :format-label="formatLabel" :sector-size="sectorSize"
        @select="emit('select', $event)" @open="emit('navigate', $event.key)" @toggle="emit('toggle', $event)" @toggle-all="emit('toggle-all')" @clear-filters="emit('clear-filters')" />
    </div>
  </div>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { ref } from 'vue';
import { ArrowUp, ChevronRight, HardDrive, Layers3, PanelLeft } from 'lucide-vue-next';
import UiTransition from '../../../components/ui/UiTransition.vue';
import RecoveryTreeRail from './RecoveryTreeRail.vue';
import RecoveryFileTable from './RecoveryFileTable.vue';
import { RECOVERY_ROOT, type RecoveryEntry } from '../model/recoveryTree';
import type { RecoveryTreeRow } from '../composables/useRecoveryBrowser';
defineProps<{
  treeRows: RecoveryTreeRow[]; expanded: ReadonlySet<string>; currentKey: string; breadcrumbs: RecoveryEntry[];
  rows: RecoveryEntry[]; signatureColumns: boolean; selectedKey: string | null; tableContext: string;
  isChecked: (item: RecoveryEntry) => boolean; isPartial: (item: RecoveryEntry) => boolean;
  allChecked: boolean; someChecked: boolean; busy: boolean; loading: boolean; filtered: boolean; emptyText: string;
  formatLabel: (formatId: string) => string; sectorSize: number;
}>();
const emit = defineEmits<{
  navigate: [key: string]; 'toggle-expanded': [key: string]; 'go-up': []; select: [item: RecoveryEntry];
  toggle: [item: RecoveryEntry]; 'toggle-all': []; 'clear-filters': [];
}>();
const treeVisible = ref(true);
</script>
<style scoped>
.recovery-browser { position: relative; display: flex; flex: 1; min-height: 0; overflow: hidden; }
.browser-main { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; }
.path-bar { display: flex; align-items: center; gap: 4px; min-height: 44px; padding: 6px 12px; flex-shrink: 0; border-bottom: 1px solid var(--border-color); background: var(--surface-secondary); }
.breadcrumb { flex: 1 1 180px; min-width: 0; height: 30px; display: flex; align-items: center; gap: 2px; margin-left: 4px; overflow: hidden; color: var(--text-secondary); white-space: nowrap; }
.crumb { min-width: 32px; max-width: 200px; height: 28px; display: inline-flex; align-items: center; gap: 6px; flex-shrink: 1; padding: 0 8px; overflow: hidden; border: none; border-radius: var(--radius-sm); background: transparent; color: var(--text-secondary); font-family: var(--font-ui); font-size: 13px; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; transition: background var(--transition-fast), color var(--transition-fast); }
.crumb:hover { background: var(--hover-bg); color: var(--text-primary); }
.crumb.root { flex-shrink: 0; }
.crumb.active { flex-shrink: 0; max-width: 50%; color: var(--text-primary); font-weight: 600; }
.crumb.deleted { color: var(--sev-critical-fg); text-decoration: line-through; }
.crumb:focus-visible { outline: 2px solid var(--primary-color); outline-offset: -2px; }
.crumb-root-icon { flex-shrink: 0; color: var(--primary-color); }
.crumb-sep { flex-shrink: 0; color: var(--text-secondary); opacity: 0.6; }
.crumb-flat-icon { flex-shrink: 0; margin: 0 4px; color: var(--showall-accent); }
</style>
