<template>
  <div class="tn">
    <div
      class="tn-row"
      :class="{ hit, clickable: isContainer && count > 0 }"
      :style="{ paddingLeft: `${depth * 14 + 6}px` }"
      @click="isContainer && count > 0 ? (open = !open) : undefined"
    >
      <span class="tn-caret">
        <ChevronRight v-if="isContainer && count > 0" :size="12" :class="{ rot: open }" />
      </span>
      <span v-if="label !== null" class="tn-key">{{ label }}</span>
      <span v-if="label !== null" class="tn-colon">:</span>
      <span v-if="isContainer" class="tn-summary">{{ summary }}</span>
      <span v-else class="tn-val" :class="`t-${value.t}`">{{ valueText }}</span>
    </div>
    <template v-if="isContainer && open">
      <StructuredTreeNode
        v-for="(c, i) in visibleChildren"
        :key="i"
        :label="c.label"
        :value="c.value"
        :depth="depth + 1"
        :query="query"
      />
      <div
        v-if="hiddenCount > 0"
        class="tn-row tn-more"
        :style="{ paddingLeft: `${(depth + 1) * 14 + 6}px` }"
      >
        {{ tr("… 其余") }} {{ hiddenCount.toLocaleString() }} {{ tr("项已省略") }}
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// 「树」视图的一行：缩进 + 折叠箭头 + 键/值，容器递归渲染子项。
// 折叠状态各实例自持（深度 < 2 默认展开），与「图」视图的折叠互不影响。
import { computed, ref } from 'vue';
import { ChevronRight } from 'lucide-vue-next';
import type { PValue } from '../../model/structured/parse';

defineOptions({ name: 'StructuredTreeNode' });

const props = defineProps<{
  label: string | null;
  value: PValue;
  depth: number;
  query?: string;
}>();

const open = ref(props.depth < 2);

const isContainer = computed(() => props.value.t === 'object' || props.value.t === 'array');
const count = computed(() => {
  const v = props.value;
  if (v.t === 'object') return v.entries.length;
  if (v.t === 'array') return v.items.length;
  return 0;
});
const summary = computed(() => (props.value.t === 'array' ? `[${count.value}]` : `{${count.value}}`));

const valueText = computed(() => {
  const v = props.value;
  switch (v.t) {
    case 'string': {
      // 单行化 + 截断：取证数据里常有几百 KB 的 base64 串，整串渲染会撑爆行宽 / 卡顿。
      const s = v.v.replace(/\s+/g, ' ');
      return `"${s.length > 200 ? `${s.slice(0, 200)}…` : s}"`;
    }
    case 'number':
      return String(v.v);
    case 'boolean':
      return v.v ? 'true' : 'false';
    case 'null':
      return 'null';
    case 'object':
      return `{${v.entries.length}}`;
    case 'array':
      return `[${v.items.length}]`;
  }
});

const children = computed<{ label: string; value: PValue }[]>(() => {
  const v = props.value;
  if (v.t === 'object') return v.entries.map(([k, val]) => ({ label: k, value: val }));
  if (v.t === 'array') return v.items.map((val, i) => ({ label: String(i), value: val }));
  return [];
});

// 单层渲染行数上限——超大容器（默认展开的根可达数万项）只渲染前 CAP 项，
// 其余补一条省略行，避免一次性建出数万个 DOM 行卡死。
const CHILD_CAP = 1000;
const visibleChildren = computed(() =>
  children.value.length > CHILD_CAP ? children.value.slice(0, CHILD_CAP) : children.value,
);
const hiddenCount = computed(() => Math.max(0, children.value.length - CHILD_CAP));

const hit = computed(() => {
  const q = (props.query ?? '').trim().toLowerCase();
  if (!q) return false;
  const inLabel = props.label !== null && props.label.toLowerCase().includes(q);
  const inVal = !isContainer.value && valueText.value.toLowerCase().includes(q);
  return inLabel || inVal;
});
</script>

<style scoped>
.tn-row {
  display: flex;
  align-items: center;
  gap: 4px;
  min-height: 22px;
  padding-right: 8px;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.5;
  border-radius: var(--radius-sm);
  white-space: nowrap;
}
.tn-row.clickable {
  cursor: pointer;
}
.tn-row.clickable:hover {
  background: var(--hover-bg);
}
.tn-row.hit {
  background: var(--sev-warn-bg);
}

.tn-caret {
  width: 12px;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-secondary);
}
.tn-caret :deep(svg) {
  transition: transform var(--transition-fast);
}
.tn-caret :deep(svg.rot) {
  transform: rotate(90deg);
}

.tn-key {
  color: var(--accent-color);
}
.tn-colon {
  color: var(--text-secondary);
}
.tn-summary {
  color: var(--text-secondary);
}
.tn-more {
  color: var(--text-secondary);
  font-style: italic;
}

.t-string {
  color: var(--sev-success-fg);
  overflow: hidden;
  text-overflow: ellipsis;
}
.t-number {
  color: var(--chart-3);
}
.t-boolean {
  color: var(--chart-4);
}
.t-null {
  color: var(--text-secondary);
  font-style: italic;
}
</style>
