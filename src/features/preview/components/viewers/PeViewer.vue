<template>
  <div class="report-viewer pe-viewer">
    <template v-if="report">
      <!-- 概览：格式 / 类型 + 标签 + 关键数字 + 技术细节 -->
      <header class="report-head">
        <div class="report-icon"><Binary :size="20" /></div>
        <div class="report-title">
          <h3 class="report-name">{{ titleText }}</h3>
          <div class="report-badges">
            <span class="ui-badge sm mono">{{ report.format }}</span>
            <span class="ui-badge sm mono">{{ report.machine_label }}</span>
            <span class="ui-badge sm mono">{{ kindBadge }}</span>
            <span class="ui-badge sm mono">{{ report.subsystem_label }}</span>
            <span v-if="report.is_dotnet" class="ui-badge sm mono info">.NET / CLR</span>
            <span v-if="report.is_signed" class="ui-badge sm success"><ShieldCheck :size="11" />{{ tr("含签名块") }}</span>
            <span v-else class="ui-badge sm"><Shield :size="11" />{{ tr("无签名") }}</span>
            <span v-if="packedSuspect" class="ui-badge sm danger"><AlertTriangle :size="11" />{{ tr("高熵节区，疑似加壳") }}</span>
            <span v-if="report.overlay" class="ui-badge sm warn"><AlertTriangle :size="11" />Overlay {{ formatBytes(report.overlay.size) }}</span>
            <span v-if="report.truncated" class="ui-badge sm warn">{{ tr("读取已截断") }}</span>
          </div>
        </div>
      </header>

      <dl class="report-stats">
        <div class="report-stat"><dt>{{ tr("节区") }}</dt><dd>{{ report.sections.length }}</dd></div>
        <div class="report-stat"><dt>{{ tr("导入 DLL") }}</dt><dd>{{ report.imports.length }}</dd></div>
        <div class="report-stat"><dt>{{ tr("导出符号") }}</dt><dd>{{ report.exports?.total ?? 0 }}</dd></div>
        <div class="report-stat"><dt>{{ tr("文件大小") }}</dt><dd>{{ formatBytes(report.file_size) }}</dd></div>
      </dl>

      <dl class="report-fields">
        <div><dt>{{ tr("编译时间") }}</dt><dd>{{ report.timestamp_utc ? formatTimestamp(report.timestamp_utc) : '—' }}</dd></div>
        <div><dt>{{ tr("入口点") }}</dt><dd>{{ hex(report.entry_point) }}{{ report.entry_section ? ` (${report.entry_section})` : '' }}</dd></div>
        <div><dt>{{ tr("镜像基址") }}</dt><dd>{{ report.image_base }}</dd></div>
        <div><dt>{{ tr("镜像大小") }}</dt><dd>{{ formatBytes(report.size_of_image) }}</dd></div>
        <div><dt>{{ tr("链接器版本") }}</dt><dd>{{ report.linker_version }}</dd></div>
        <div><dt>{{ tr("目标系统版本") }}</dt><dd>{{ report.os_version }}</dd></div>
        <div><dt>{{ tr("校验和") }}</dt><dd>{{ hex(report.checksum) }}</dd></div>
        <div><dt>{{ tr("头部大小") }}</dt><dd>{{ formatBytes(report.size_of_headers) }}</dd></div>
        <div class="wide"><dt>{{ tr("文件特性") }}</dt><dd>{{ joinList(report.characteristic_flags, tr) || '—' }}</dd></div>
        <div class="wide"><dt>{{ tr("安全特性") }}</dt><dd>{{ joinList(report.dll_characteristic_flags, tr) || '—' }}</dd></div>
      </dl>

      <!-- 解析警告 -->
      <section v-if="report.parse_warnings.length" class="ui-alert warn report-warnings" role="note">
        <AlertTriangle :size="14" />
        <div class="warn-body">
          <p class="warn-title">{{ tr("解析警告") }}</p>
          <p v-for="(w, i) in report.parse_warnings" :key="i" class="warn-line mono">{{ w }}</p>
        </div>
      </section>

      <!-- 节区表 -->
      <section class="section">
        <div class="section-head">
          <h4 class="section-title">{{ tr("节区") }} <span class="ui-count">{{ report.sections.length }}</span></h4>
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
      <section v-if="report.imports.length" class="section">
        <div class="section-head">
          <h4 class="section-title">{{ tr("导入表") }} <span class="ui-count">{{ report.imports.length }}</span></h4>
          <label class="ui-search section-filter">
            <Search :size="13" aria-hidden="true" />
            <input v-model="importFilter" type="text" :placeholder="tr('过滤 DLL / 函数')" :aria-label="tr('过滤 DLL / 函数')" spellcheck="false" />
          </label>
        </div>
        <div class="framed-list">
          <details v-for="(imp, i) in filteredImports" :key="`${i}-${imp.dll}`" v-disclosure-motion class="ui-disclosure disclosure">
            <summary>
              <ChevronRight :size="13" class="ui-chevron" />
              <span class="mono strong">{{ imp.dll }}</span>
              <span v-if="imp.delayed" class="ui-badge sm warn">{{ tr("延迟导入") }}</span>
              <span class="dim">{{ imp.total }} {{ tr("函数") }}{{ imp.total > imp.functions.length ? tr("（前 {0}）", { 0: imp.functions.length }) : '' }}</span>
            </summary>
            <div class="func-list mono">
              <span v-for="(f, fi) in imp.functions" :key="fi" class="func">{{ f }}</span>
            </div>
          </details>
          <div v-if="filteredImports.length === 0" class="empty-row">{{ tr("无匹配项") }}</div>
        </div>
      </section>

      <!-- 导出表 -->
      <section v-if="report.exports && report.exports.entries.length" class="section">
        <div class="section-head">
          <h4 class="section-title">
            {{ tr("导出表") }} <span class="ui-count">{{ report.exports.total }}</span>
            <span v-if="report.exports.dll_name" class="dim mono">{{ report.exports.dll_name }}</span>
          </h4>
          <label class="ui-search section-filter">
            <Search :size="13" aria-hidden="true" />
            <input v-model="exportFilter" type="text" :placeholder="tr('过滤名称 / 序号')" :aria-label="tr('过滤名称 / 序号')" spellcheck="false" />
          </label>
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
      <section v-if="report.version_info" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("版本信息") }}</h4></div>
        <dl class="report-fields">
          <div><dt>{{ tr("文件版本") }}</dt><dd>{{ report.version_info.file_version ?? '—' }}</dd></div>
          <div><dt>{{ tr("产品版本") }}</dt><dd>{{ report.version_info.product_version ?? '—' }}</dd></div>
          <div v-for="(s, si) in report.version_info.strings" :key="`${si}-${s.key}`" :class="{ wide: s.value.length > 40 }">
            <dt>{{ s.key }}</dt><dd>{{ s.value || '—' }}</dd>
          </div>
        </dl>
      </section>

      <!-- 调试目录 -->
      <section v-if="report.debug.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("调试目录") }}</h4></div>
        <div v-for="(d, i) in report.debug" :key="i" class="debug-entry">
          <span class="ui-badge sm mono">{{ d.kind }}</span>
          <span v-if="d.pdb_path" class="mono pdb-path">{{ d.pdb_path }}</span>
          <span v-if="d.guid" class="mono dim">GUID {{ d.guid }}{{ d.age != null ? ` · Age ${d.age}` : '' }}</span>
        </div>
      </section>

      <!-- 数据目录 -->
      <section class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("数据目录") }}</h4></div>
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
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';
import { joinList } from '../../../../i18n/translate';

import { vDisclosureMotion } from '../../../../composables/uiMotionDirectives';
// PE 文件的「原类型视图」：由 BinaryViewer 传入已解析的 PeReport，按取证视角渲染
// （编译时间 / 节区熵 / 导入导出 / PDB 路径 / 版本资源 / overlay）。
// 版式与其他结构化报告一致：概览头 + 关键数字 + 平铺的分节。
import { computed, ref } from 'vue';
import { AlertTriangle, Binary, ChevronRight, Search, Shield, ShieldCheck } from 'lucide-vue-next';
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
}
.pdb-path {
  color: var(--text-primary);
  word-break: break-all;
}
</style>
