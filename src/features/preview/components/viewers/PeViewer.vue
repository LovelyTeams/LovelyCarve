<template>
  <div class="report-viewer pe-viewer">
    <template v-if="report">
      <!-- 概览：格式 / 类型 + 标签 + 关键数字 + 技术细节 -->
      <section class="card overview">
        <div class="title-row">
          <div class="title-icon"><Binary :size="22" /></div>
          <div class="title-text">
            <h3 class="model-name">{{ titleText }}</h3>
            <div class="chips">
              <span class="chip accent">{{ report.format }}</span>
              <span class="chip">{{ report.machine_label }}</span>
              <span class="chip">{{ kindBadge }}</span>
              <span class="chip">{{ report.subsystem_label }}</span>
              <span v-if="report.is_dotnet" class="chip info">.NET / CLR</span>
              <span v-if="report.is_signed" class="chip success"><ShieldCheck :size="11" /> {{ tr("含签名块") }}</span>
              <span v-else class="chip"><Shield :size="11" /> {{ tr("无签名") }}</span>
              <span v-if="packedSuspect" class="chip critical"><AlertTriangle :size="11" /> {{ tr("高熵节区，疑似加壳") }}</span>
              <span v-if="report.overlay" class="chip warn"><AlertTriangle :size="11" /> Overlay {{ formatBytes(report.overlay.size) }}</span>
              <span v-if="report.truncated" class="chip warn">{{ tr("读取已截断") }}</span>
            </div>
          </div>
        </div>

        <div class="tiles">
          <div class="tile">
            <span class="tile-num">{{ report.sections.length }}</span>
            <span class="tile-label">{{ tr("节区") }}</span>
          </div>
          <div class="tile">
            <span class="tile-num">{{ report.imports.length }}</span>
            <span class="tile-label">{{ tr("导入 DLL") }}</span>
          </div>
          <div class="tile">
            <span class="tile-num">{{ report.exports?.total ?? 0 }}</span>
            <span class="tile-label">{{ tr("导出符号") }}</span>
          </div>
          <div class="tile">
            <span class="tile-num">{{ formatBytes(report.file_size) }}</span>
            <span class="tile-label">{{ tr("文件大小") }}</span>
          </div>
        </div>

        <div class="detail-grid">
          <div><span class="dk">{{ tr("编译时间") }}</span><span class="dv mono">{{ report.timestamp_utc ? formatTimestamp(report.timestamp_utc) : '—' }}</span></div>
          <div><span class="dk">{{ tr("入口点") }}</span><span class="dv mono">{{ hex(report.entry_point) }}{{ report.entry_section ? ` (${report.entry_section})` : '' }}</span></div>
          <div><span class="dk">{{ tr("镜像基址") }}</span><span class="dv mono">{{ report.image_base }}</span></div>
          <div><span class="dk">{{ tr("镜像大小") }}</span><span class="dv mono">{{ formatBytes(report.size_of_image) }}</span></div>
          <div><span class="dk">{{ tr("链接器版本") }}</span><span class="dv mono">{{ report.linker_version }}</span></div>
          <div><span class="dk">{{ tr("目标系统版本") }}</span><span class="dv mono">{{ report.os_version }}</span></div>
          <div><span class="dk">{{ tr("校验和") }}</span><span class="dv mono">{{ hex(report.checksum) }}</span></div>
          <div><span class="dk">{{ tr("头部大小") }}</span><span class="dv mono">{{ formatBytes(report.size_of_headers) }}</span></div>
          <div class="wide"><span class="dk">{{ tr("文件特性") }}</span><span class="dv mono">{{ joinList(report.characteristic_flags, tr) || '—' }}</span></div>
          <div class="wide"><span class="dk">{{ tr("安全特性") }}</span><span class="dv mono">{{ joinList(report.dll_characteristic_flags, tr) || '—' }}</span></div>
        </div>
      </section>

      <!-- 节区表 -->
      <section class="card">
        <div class="card-head">
          <h4 class="card-title">{{ tr("节区") }} <span class="count-badge mono">{{ report.sections.length }}</span></h4>
        </div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead>
              <tr><th>{{ tr("名称") }}</th><th>{{ tr("虚拟地址") }}</th><th>{{ tr("虚拟大小") }}</th><th>{{ tr("文件偏移") }}</th><th>{{ tr("文件大小") }}</th><th>{{ tr("权限") }}</th><th class="th-entropy">{{ tr("熵") }}</th></tr>
            </thead>
            <tbody>
              <tr
                v-for="s in report.sections"
                :key="s.name + s.virtual_address"
                :class="{ 'row-flag': s.is_code && s.entropy != null && s.entropy > 7.2 }"
              >
                <td class="mono strong">{{ s.name || tr("（未命名）") }}</td>
                <td class="mono">{{ hex(s.virtual_address) }}</td>
                <td class="mono">{{ hex(s.virtual_size) }}</td>
                <td class="mono">{{ hex(s.raw_offset) }}</td>
                <td class="mono">{{ hex(s.raw_size) }}</td>
                <td class="mono">{{ s.perms }}{{ s.is_code ? ' · code' : '' }}</td>
                <td>
                  <div v-if="s.entropy != null" class="entropy-cell">
                    <span class="entropy-bar"><span class="entropy-fill" :class="entropyClass(s.entropy)" :style="{ width: `${(s.entropy / 8) * 100}%` }" /></span>
                    <span class="mono dim">{{ s.entropy.toFixed(2) }}</span>
                  </div>
                  <span v-else class="mono dim">—</span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <!-- 导入表 -->
      <section v-if="report.imports.length" class="card">
        <div class="card-head">
          <h4 class="card-title">{{ tr("导入表") }} <span class="count-badge mono">{{ report.imports.length }} DLL</span></h4>
          <div class="filter">
            <Search :size="12" />
            <input v-model="importFilter" type="text" :placeholder="tr('过滤 DLL / 函数')" spellcheck="false" />
          </div>
        </div>
        <details v-disclosure-motion v-for="(imp, i) in filteredImports" :key="`${i}-${imp.dll}`" class="disclosure">
          <summary>
            <span class="mono strong">{{ imp.dll }}</span>
            <span v-if="imp.delayed" class="chip warn">{{ tr("延迟导入") }}</span>
            <span class="dim">{{ imp.total }} {{ tr("函数") }}{{ imp.total > imp.functions.length ? tr("（前 {0}）", { 0: imp.functions.length }) : '' }}</span>
          </summary>
          <div class="func-list mono">
            <span v-for="(f, fi) in imp.functions" :key="fi" class="func">{{ f }}</span>
          </div>
        </details>
        <div v-if="filteredImports.length === 0" class="empty-row">{{ tr("无匹配项") }}</div>
      </section>

      <!-- 导出表 -->
      <section v-if="report.exports && report.exports.entries.length" class="card">
        <div class="card-head">
          <h4 class="card-title">
            {{ tr("导出表") }} <span class="count-badge mono">{{ report.exports.total }}</span>
            <span v-if="report.exports.dll_name" class="dim mono">· {{ report.exports.dll_name }}</span>
          </h4>
          <div class="filter">
            <Search :size="12" />
            <input v-model="exportFilter" type="text" :placeholder="tr('过滤名称 / 序号')" spellcheck="false" />
          </div>
        </div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead><tr><th class="ralign">{{ tr("序号") }}</th><th>{{ tr("名称") }}</th><th>RVA</th></tr></thead>
            <tbody>
              <tr v-for="(e, ei) in filteredExports" :key="ei">
                <td class="mono ralign">{{ e.ordinal }}</td>
                <td class="mono">{{ e.name ?? tr("（仅序号）") }}<span v-if="e.forwarder" class="dim"> → {{ e.forwarder }}</span></td>
                <td class="mono">{{ hex(e.rva) }}</td>
              </tr>
              <tr v-if="filteredExports.length === 0">
                <td colspan="3" class="empty-row">{{ tr("无匹配项") }}</td>
              </tr>
            </tbody>
          </table>
        </div>
        <div v-if="report.exports.total > report.exports.entries.length" class="trunc-note mono">{{ tr("已截断，仅显示前") }} {{ report.exports.entries.length }} {{ tr("项") }}</div>
      </section>

      <!-- 版本信息 -->
      <section v-if="report.version_info" class="card">
        <div class="card-head"><h4 class="card-title">{{ tr("版本信息") }}</h4></div>
        <div class="detail-grid">
          <div><span class="dk">{{ tr("文件版本") }}</span><span class="dv mono">{{ report.version_info.file_version ?? '—' }}</span></div>
          <div><span class="dk">{{ tr("产品版本") }}</span><span class="dv mono">{{ report.version_info.product_version ?? '—' }}</span></div>
          <div v-for="(s, si) in report.version_info.strings" :key="`${si}-${s.key}`" :class="{ wide: s.value.length > 40 }">
            <span class="dk">{{ s.key }}</span><span class="dv mono">{{ s.value || '—' }}</span>
          </div>
        </div>
      </section>

      <!-- 调试目录 -->
      <section v-if="report.debug.length" class="card">
        <div class="card-head"><h4 class="card-title">{{ tr("调试目录") }}</h4></div>
        <div v-for="(d, i) in report.debug" :key="i" class="debug-entry">
          <span class="chip">{{ d.kind }}</span>
          <span v-if="d.pdb_path" class="mono pdb-path">{{ d.pdb_path }}</span>
          <span v-if="d.guid" class="mono dim">GUID {{ d.guid }}{{ d.age != null ? ` · Age ${d.age}` : '' }}</span>
        </div>
      </section>

      <!-- 数据目录 -->
      <section class="card">
        <div class="card-head"><h4 class="card-title">{{ tr("数据目录") }}</h4></div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead><tr><th>{{ tr("目录") }}</th><th>RVA</th><th>{{ tr("大小") }}</th></tr></thead>
            <tbody>
              <tr v-for="d in report.data_directories" :key="d.name" :class="{ dim: !d.rva && !d.size }">
                <td class="mono">{{ d.name }}</td>
                <td class="mono">{{ d.rva ? hex(d.rva) : '—' }}</td>
                <td class="mono">{{ d.size ? hex(d.size) : '—' }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <!-- 解析警告 -->
      <section v-if="report.parse_warnings.length" class="card warn-card">
        <div class="card-head"><h4 class="card-title"><AlertTriangle :size="13" /> {{ tr("解析警告") }}</h4></div>
        <div v-for="(w, i) in report.parse_warnings" :key="i" class="warn-line mono">{{ w }}</div>
      </section>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';
import { joinList } from '../../../../i18n/translate';

import { vDisclosureMotion } from '../../../../composables/uiMotionDirectives';
// PE 文件的「原类型视图」：由 BinaryViewer 传入已解析的 PeReport，按取证视角渲染
// （编译时间 / 节区熵 / 导入导出 / PDB 路径 / 版本资源 / overlay）。
// 视觉与 GgufViewer 一致：概览头 + 数据块 + 卡片化表格。
import { computed, ref } from 'vue';
import { AlertTriangle, Binary, Search, Shield, ShieldCheck } from 'lucide-vue-next';
import type { PeReport } from '../../../../lib/ipc/binary';
import { formatBytes } from '../../../../utils/format';
import { formatTimestamp } from '../../../../utils/time';

const props = defineProps<{
  report: PeReport;
}>();

const importFilter = ref('');
const exportFilter = ref('');

// 模板沿用 `report.*` 写法，这里把 prop 暴露为同名计算属性。
const report = computed(() => props.report);

const kindBadge = computed(() => {
  const r = props.report;
  if (r.is_dll) return 'DLL';
  if (r.subsystem === 1) return tr("驱动 / Native");
  return 'EXE';
});

// 概览标题：优先用版本资源里的描述 / 产品名 / 原始文件名（注意可被伪造，权威
// 信息看下方标签），都没有时退回类型 / 格式。
const titleText = computed(() => {
  const vi = props.report.version_info;
  const pick = (k: string) => vi?.strings.find((s) => s.key === k)?.value?.trim();
  return (
    pick('FileDescription') ||
    pick('ProductName') ||
    pick('OriginalFilename') ||
    kindBadge.value ||
    props.report.format ||
    'PE'
  );
});

// 代码节熵 > 7.2 是典型的压缩/加密特征（UPX 等壳）。
const packedSuspect = computed(() =>
  props.report.sections.some((s) => s.is_code && s.entropy != null && s.entropy > 7.2),
);

function hex(n: number): string {
  return `0x${n.toString(16)}`;
}

function entropyClass(e: number): string {
  if (e > 7.2) return 'high';
  if (e > 6.5) return 'mid';
  return 'low';
}

const filteredImports = computed(() => {
  const q = importFilter.value.trim().toLowerCase();
  if (!q) return props.report.imports;
  return props.report.imports.filter(
    (imp) => imp.dll.toLowerCase().includes(q) || imp.functions.some((f) => f.toLowerCase().includes(q)),
  );
});

const filteredExports = computed(() => {
  const ex = props.report.exports;
  if (!ex) return [];
  const q = exportFilter.value.trim().toLowerCase();
  if (!q) return ex.entries;
  return ex.entries.filter(
    (e) => (e.name ?? '').toLowerCase().includes(q) || String(e.ordinal).includes(q),
  );
});
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
/* ── 调试目录 ── */
.debug-entry {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  font-size: 12px;
  padding: 3px 0;
}
.pdb-path {
  color: var(--text-primary);
  word-break: break-all;
}
</style>
