<template>
  <div class="report-viewer gguf-viewer">
    <div v-if="loading" class="state-row mono"><Loader2 :size="14" class="spin" /> {{ tr("正在解析 GGUF 结构…") }}</div>
    <div v-else-if="error" class="state-row mono error">{{ error }}</div>

    <template v-else-if="report">
      <!-- 概览：模型名 + 标签 + 关键数字 + 技术细节 -->
      <section class="card overview">
        <div class="title-row">
          <div class="title-icon"><Boxes :size="22" /></div>
          <div class="title-text">
            <h3 class="model-name">{{ displayName }}</h3>
            <div class="chips">
              <span class="chip accent">GGUF v{{ report.version }}</span>
              <span v-if="report.architecture" class="chip">{{ report.architecture }}</span>
              <span v-if="report.quantization" class="chip">{{ report.quantization }}</span>
            </div>
          </div>
        </div>

        <div class="tiles">
          <div class="tile">
            <span class="tile-num">{{ humanParams }}</span>
            <span class="tile-label">{{ tr("参数量") }}</span>
          </div>
          <div class="tile">
            <span class="tile-num">{{ report.tensor_count.toLocaleString() }}</span>
            <span class="tile-label">{{ tr("张量") }}</span>
          </div>
          <div class="tile">
            <span class="tile-num">{{ report.metadata_kv_count.toLocaleString() }}</span>
            <span class="tile-label">{{ tr("元数据项") }}</span>
          </div>
          <div class="tile">
            <span class="tile-num">{{ report.quantization || '—' }}</span>
            <span class="tile-label">{{ tr("量化") }}</span>
          </div>
          <div class="tile">
            <span class="tile-num">{{ formatBytes(report.file_size) }}</span>
            <span class="tile-label">{{ tr("文件大小") }}</span>
          </div>
        </div>

        <div class="detail-grid">
          <div><span class="dk">{{ tr("架构") }}</span><span class="dv mono">{{ report.architecture || '—' }}</span></div>
          <div><span class="dk">{{ tr("量化") }}</span><span class="dv mono">{{ quantLabel }}</span></div>
          <div><span class="dk">{{ tr("完整参数量") }}</span><span class="dv mono">{{ report.total_parameters.toLocaleString() }}</span></div>
          <div><span class="dk">{{ tr("对齐") }}</span><span class="dv mono">{{ report.alignment }} {{ tr("字节") }}</span></div>
          <div><span class="dk">{{ tr("张量数据偏移") }}</span><span class="dv mono">{{ formatBytes(report.tensor_data_offset) }} · 0x{{ report.tensor_data_offset.toString(16).toUpperCase() }}</span></div>
          <div><span class="dk">{{ tr("GGUF 版本") }}</span><span class="dv mono">v{{ report.version }}</span></div>
        </div>
      </section>

      <!-- 张量类型分布 -->
      <section v-if="report.tensor_types.length" class="card">
        <div class="card-head">
          <h4 class="card-title">{{ tr("张量类型分布") }}</h4>
        </div>
        <div class="share-bar" v-if="report.total_parameters > 0">
          <span
            v-for="(t, i) in report.tensor_types"
            :key="t.type_name"
            class="share-seg"
            :style="{ width: `${(t.parameters / report.total_parameters) * 100}%`, background: typeColor(i) }"
            :title="`${t.type_name} · ${paramPct(t.parameters)}`"
          ></span>
        </div>
        <div class="type-chips">
          <div v-for="(t, i) in report.tensor_types" :key="t.type_name" class="type-chip">
            <span class="type-dot" :style="{ background: typeColor(i) }"></span>
            <span class="type-name mono">{{ t.type_name }}</span>
            <span class="type-meta mono">{{ t.count }} {{ tr("个 ·") }} {{ paramPct(t.parameters) }}</span>
          </div>
        </div>
      </section>

      <!-- 元数据 -->
      <section class="card">
        <div class="card-head">
          <h4 class="card-title">{{ tr("元数据") }} <span class="count-badge mono">{{ report.metadata.length }}</span></h4>
          <div class="filter">
            <Search :size="12" />
            <input v-model="metaFilter" type="text" :placeholder="tr('过滤键 / 值')" spellcheck="false" />
          </div>
        </div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead>
              <tr><th class="col-key">{{ tr("键") }}</th><th class="col-type">{{ tr("类型") }}</th><th>{{ tr("值") }}</th></tr>
            </thead>
            <tbody>
              <tr v-for="m in filteredMetadata" :key="m.key">
                <td class="meta-key">{{ m.key }}</td>
                <td class="meta-type">{{ m.type_name }}</td>
                <td class="meta-val">
                  <template v-if="m.array_len !== null">
                    <span class="arr-badge mono">[{{ m.array_len.toLocaleString() }}]</span>
                    <span class="arr-preview" :title="m.array_preview?.join(', ')">{{ arrayPreviewText(m) }}</span>
                  </template>
                  <span v-else class="scalar" :title="m.value">{{ m.value || '—' }}</span>
                </td>
              </tr>
              <tr v-if="filteredMetadata.length === 0">
                <td colspan="3" class="empty-row">{{ tr("无匹配项") }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <!-- 张量 -->
      <section class="card">
        <div class="card-head">
          <h4 class="card-title">
            {{ tr("张量") }}
            <span class="count-badge mono">{{ report.tensors_truncated ? `${report.tensors.length} / ${report.tensor_count}` : report.tensors.length }}</span>
          </h4>
          <div class="filter">
            <Search :size="12" />
            <input v-model="tensorFilter" type="text" :placeholder="tr('过滤名称 / 类型')" spellcheck="false" />
          </div>
        </div>
        <div v-if="report.tensors.length === 0" class="empty-row pad">{{ tr("该文件不含张量。") }}</div>
        <template v-else>
          <div class="table-scroll">
            <table class="data-table g-table">
              <thead>
                <tr><th>{{ tr("名称") }}</th><th>{{ tr("类型") }}</th><th>{{ tr("形状") }}</th><th class="ralign">{{ tr("参数量") }}</th><th class="ralign">{{ tr("偏移") }}</th></tr>
              </thead>
              <tbody>
                <tr v-for="(t, i) in filteredTensors" :key="`${i}-${t.name}`">
                  <td class="t-name mono">{{ t.name }}</td>
                  <td><span class="t-type">{{ t.type_name }}</span></td>
                  <td class="mono t-shape">{{ t.dimensions.length ? t.dimensions.join(' × ') : tr("标量") }}</td>
                  <td class="mono ralign">{{ t.parameters.toLocaleString() }}</td>
                  <td class="mono ralign dim">0x{{ t.offset.toString(16).toUpperCase() }}</td>
                </tr>
                <tr v-if="filteredTensors.length === 0">
                  <td colspan="5" class="empty-row">{{ tr("无匹配项") }}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <div v-if="report.tensors_truncated" class="trunc-note mono">
            {{ tr("张量较多，仅显示前") }} {{ report.tensors.length.toLocaleString() }} {{ tr("个（共") }} {{ report.tensor_count.toLocaleString() }}{{ tr("）；上方参数量 / 类型分布按全部张量统计。") }}
          </div>
        </template>
      </section>

      <!-- 解析警告 -->
      <section v-if="report.parse_warnings.length" class="card warn-card">
        <div class="card-head">
          <h4 class="card-title"><AlertTriangle :size="13" /> {{ tr("解析警告") }}</h4>
        </div>
        <div v-for="(w, i) in report.parse_warnings" :key="i" class="warn-line mono">{{ w }}</div>
      </section>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// GGUF（llama.cpp / ggml 大模型权重容器）的「原类型视图」：调用 fs_gguf_preview
// 拿结构化报告，按模型来源视角渲染（架构 / 量化 / 参数量 / 张量类型分布 / 完整
// 元数据 / 张量布局）。只读文件前部头区，不触碰数 GB 的权重数据本体。
import { computed, ref, watch } from 'vue';
import { AlertTriangle, Boxes, Loader2, Search } from 'lucide-vue-next';
import { fsGgufPreview, type GgufMetaEntry, type GgufReport } from '../../../../lib/ipc/gguf';
import { formatBytes } from '../../../../utils/format';

const props = defineProps<{
  imageId: string;
  nodeId: string;
}>();

const loading = ref(false);
const error = ref<string | null>(null);
const report = ref<GgufReport | null>(null);
const metaFilter = ref('');
const tensorFilter = ref('');

const displayName = computed(
  () => report.value?.name || report.value?.architecture || tr("未命名 GGUF 模型"),
);

const quantLabel = computed(() => {
  const r = report.value;
  if (!r || !r.quantization) return '—';
  return r.file_type !== null ? `${r.quantization}（ID ${r.file_type}）` : r.quantization;
});

// 参数量人性化：663000000 → "663M"，7000000000 → "7B"。
function compactCount(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '—';
  const units: [number, string][] = [
    [1e12, 'T'],
    [1e9, 'B'],
    [1e6, 'M'],
    [1e3, 'K'],
  ];
  for (const [scale, suffix] of units) {
    if (n >= scale) {
      const v = n / scale;
      const digits = v >= 100 ? 0 : v >= 10 ? 1 : 2;
      return v.toFixed(digits).replace(/\.?0+$/, '') + suffix;
    }
  }
  return String(n);
}

const humanParams = computed(() => (report.value ? compactCount(report.value.total_parameters) : ''));

// 张量类型分布配色：循环用主题的分类色板。
function typeColor(i: number): string {
  return `var(--chart-${(i % 6) + 1})`;
}

function paramPct(parameters: number): string {
  const total = report.value?.total_parameters ?? 0;
  if (!total) return '—';
  return `${((parameters / total) * 100).toFixed(1)}%`;
}

function arrayPreviewText(m: GgufMetaEntry): string {
  const preview = m.array_preview ?? [];
  const more = (m.array_len ?? 0) > preview.length;
  return preview.join(', ') + (more ? ' …' : '');
}

const filteredMetadata = computed<GgufMetaEntry[]>(() => {
  const r = report.value;
  if (!r) return [];
  const q = metaFilter.value.trim().toLowerCase();
  if (!q) return r.metadata;
  return r.metadata.filter(
    (m) => m.key.toLowerCase().includes(q) || m.value.toLowerCase().includes(q),
  );
});

const filteredTensors = computed(() => {
  const r = report.value;
  if (!r) return [];
  const q = tensorFilter.value.trim().toLowerCase();
  if (!q) return r.tensors;
  return r.tensors.filter(
    (t) => t.name.toLowerCase().includes(q) || t.type_name.toLowerCase().includes(q),
  );
});

async function load() {
  loading.value = true;
  error.value = null;
  report.value = null;
  metaFilter.value = '';
  tensorFilter.value = '';
  try {
    report.value = await fsGgufPreview({ imageId: props.imageId, nodeId: props.nodeId });
  } catch (err) {
    error.value = String(err);
  } finally {
    loading.value = false;
  }
}

watch(() => [props.imageId, props.nodeId], load, { immediate: true });
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
/* ── 张量类型分布 ── */
.share-bar {
  display: flex;
  height: 8px;
  border-radius: 999px;
  overflow: hidden;
  margin-bottom: 12px;
  background: var(--surface-tertiary);
}
.share-seg {
  height: 100%;
  min-width: 2px;
}
.type-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 16px;
}
.type-chip {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
}
.type-dot {
  width: 10px;
  height: 10px;
  border-radius: 3px;
  flex: none;
}
.type-name {
  color: var(--text-primary);
  font-weight: 600;
}
.type-meta {
  color: var(--text-secondary);
  font-size: 11px;
}

.col-key {
  width: 34%;
}
.col-type {
  width: 96px;
}

.meta-key {
  font-family: var(--font-mono);
  color: var(--text-primary);
  word-break: break-all;
}
.meta-type {
  font-family: var(--font-mono);
  color: var(--text-secondary);
  white-space: nowrap;
}
.meta-val {
  font-family: var(--font-mono);
  max-width: 0;
}
.arr-badge {
  color: var(--primary-color);
  margin-right: 6px;
}
.meta-val .scalar,
.meta-val .arr-preview {
  display: inline-block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
  color: var(--text-secondary);
}
.t-name {
  color: var(--text-primary);
  word-break: break-all;
}
.t-type {
  display: inline-block;
  font-family: var(--font-mono);
  font-size: 11px;
  padding: 1px 6px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-color);
  background: var(--surface-tertiary);
  color: var(--text-primary);
  white-space: nowrap;
}
.t-shape {
  color: var(--text-secondary);
  white-space: nowrap;
}
.empty-row.pad {
  padding: 18px;
}
</style>
