<template>
  <div class="report-viewer elf-viewer">
    <template v-if="report">
      <!-- 概览 -->
      <header class="report-head">
        <div class="report-icon"><Binary :size="20" /></div>
        <div class="report-title">
          <h3 class="report-name">{{ report.elf_type }}</h3>
          <div class="report-badges">
            <span class="ui-badge sm mono">{{ report.format }}</span>
            <span class="ui-badge sm mono">{{ report.machine_label }}</span>
            <span class="ui-badge sm mono">{{ report.endianness }}</span>
            <span class="ui-badge sm mono">{{ report.os_abi_label }}</span>
            <span v-for="f in report.security_flags" :key="f" class="ui-badge sm" :class="sevClass(f)">{{ f }}</span>
            <span v-if="report.truncated" class="ui-badge sm warn">{{ tr("读取已截断") }}</span>
          </div>
        </div>
      </header>

      <dl class="report-stats">
        <div class="report-stat"><dt>{{ tr("节") }}</dt><dd>{{ report.sections.length }}</dd></div>
        <div class="report-stat"><dt>{{ tr("段") }}</dt><dd>{{ report.program_headers.length }}</dd></div>
        <div class="report-stat"><dt>{{ tr("依赖库") }}</dt><dd>{{ report.needed.length }}</dd></div>
        <div class="report-stat"><dt>{{ tr("文件大小") }}</dt><dd>{{ formatBytes(report.file_size) }}</dd></div>
      </dl>

      <dl class="report-fields">
        <div><dt>{{ tr("入口") }}</dt><dd>{{ report.entry }}</dd></div>
        <div><dt>ABI</dt><dd>{{ report.os_abi_label }}</dd></div>
        <div v-if="report.interpreter" class="wide"><dt>{{ tr("解释器") }}</dt><dd>{{ report.interpreter }}</dd></div>
        <div v-if="report.soname" class="wide"><dt>SONAME</dt><dd>{{ report.soname }}</dd></div>
        <div v-if="report.build_id" class="wide"><dt>Build ID</dt><dd>{{ report.build_id }}</dd></div>
      </dl>

      <!-- 解析警告 -->
      <section v-if="report.parse_warnings.length" class="ui-alert warn report-warnings" role="note">
        <AlertTriangle :size="14" />
        <div class="warn-body">
          <p class="warn-title">{{ tr("解析警告") }}</p>
          <p v-for="(w, i) in report.parse_warnings" :key="i" class="warn-line mono">{{ w }}</p>
        </div>
      </section>

      <!-- 依赖库 -->
      <section v-if="report.needed.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("依赖库（DT_NEEDED）") }} <span class="ui-count">{{ report.needed.length }}</span></h4></div>
        <div class="tag-list">
          <span v-for="(lib, i) in report.needed" :key="i" class="tag">{{ lib }}</span>
        </div>
      </section>

      <!-- 运行时库搜索路径 -->
      <section v-if="report.rpaths.length || report.runpaths.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("运行时库搜索路径") }}</h4></div>
        <dl class="report-fields">
          <div v-for="(p, i) in report.runpaths" :key="`run-${i}`" class="wide"><dt>RUNPATH</dt><dd>{{ p }}</dd></div>
          <div v-for="(p, i) in report.rpaths" :key="`rp-${i}`" class="wide"><dt>RPATH</dt><dd>{{ p }}</dd></div>
        </dl>
      </section>

      <!-- 程序头（段） -->
      <section v-if="report.program_headers.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("程序头（段）") }} <span class="ui-count">{{ report.program_headers.length }}</span></h4></div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead><tr><th>{{ tr("类型") }}</th><th>{{ tr("权限") }}</th><th>{{ tr("文件偏移") }}</th><th>{{ tr("虚拟地址") }}</th><th>{{ tr("文件大小") }}</th><th>{{ tr("内存大小") }}</th></tr></thead>
            <tbody>
              <tr v-for="(p, i) in report.program_headers" :key="i">
                <td class="mono strong">{{ p.ptype }}</td>
                <td class="mono">{{ p.perms }}</td>
                <td class="mono">{{ hex(p.offset) }}</td>
                <td class="mono">{{ p.vaddr }}</td>
                <td class="mono">{{ hex(p.filesz) }}</td>
                <td class="mono">{{ hex(p.memsz) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <!-- 节（含熵） -->
      <section v-if="report.sections.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("节") }} <span class="ui-count">{{ report.sections.length }}</span></h4></div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead><tr><th>{{ tr("名称") }}</th><th>{{ tr("类型") }}</th><th>{{ tr("标志") }}</th><th>{{ tr("地址") }}</th><th>{{ tr("文件偏移") }}</th><th>{{ tr("大小") }}</th><th class="th-entropy">{{ tr("熵") }}</th></tr></thead>
            <tbody>
              <tr v-for="(s, i) in report.sections" :key="i" :class="{ 'row-flag': s.entropy != null && s.entropy > 7.2 }">
                <td class="mono strong">{{ s.name || tr("（未命名）") }}</td>
                <td class="mono">{{ s.sh_type }}</td>
                <td class="mono">{{ s.flags }}</td>
                <td class="mono">{{ s.addr }}</td>
                <td class="mono">{{ hex(s.offset) }}</td>
                <td class="mono">{{ hex(s.size) }}</td>
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

      <!-- 符号摘要 -->
      <section class="section">
        <div class="section-head">
          <h4 class="section-title">{{ tr("符号摘要") }}</h4>
          <label v-if="report.symbol_summary.exported.length || report.symbol_summary.imported.length" class="ui-search section-filter">
            <Search :size="13" aria-hidden="true" />
            <input v-model="symFilter" type="text" :placeholder="tr('过滤符号')" :aria-label="tr('过滤符号')" spellcheck="false" />
          </label>
        </div>
        <dl class="report-fields">
          <div><dt>{{ tr("动态符号") }}</dt><dd>{{ report.symbol_summary.dynsym_count }}</dd></div>
          <div><dt>{{ tr("调试符号") }}</dt><dd>{{ report.symbol_summary.symtab_count }}{{ report.symbol_summary.stripped ? tr(" · 已剥离") : '' }}</dd></div>
        </dl>
        <div class="sym-cols">
          <div v-if="report.symbol_summary.exported.length" class="sym-col">
            <div class="sym-head">{{ tr("导出符号") }} <span class="ui-count">{{ report.symbol_summary.exported.length }}{{ report.symbol_summary.exported_truncated ? '+' : '' }}</span></div>
            <div class="func-list mono"><span v-for="(s, i) in filteredExported" :key="i" class="func">{{ s }}</span></div>
          </div>
          <div v-if="report.symbol_summary.imported.length" class="sym-col">
            <div class="sym-head">{{ tr("导入符号") }} <span class="ui-count">{{ report.symbol_summary.imported.length }}{{ report.symbol_summary.imported_truncated ? '+' : '' }}</span></div>
            <div class="func-list mono"><span v-for="(s, i) in filteredImported" :key="i" class="func">{{ s }}</span></div>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// ELF 文件的「原类型视图」：由 BinaryViewer 传入已解析的 ElfReport，按取证视角
// 渲染（机器/ABI、安全缓解、解释器/soname/依赖、build-id、段/节熵、符号摘要）。
// 版式与 PeViewer 一致。
import { computed, ref } from 'vue';
import { AlertTriangle, Binary, Search } from 'lucide-vue-next';
import type { ElfReport } from '../../../../lib/ipc/binary';
import { formatBytes } from '../../../../utils/format';

const props = defineProps<{
  report: ElfReport;
}>();

const report = computed(() => props.report);
const symFilter = ref('');

function hex(n: number): string {
  return `0x${n.toString(16)}`;
}

function entropyClass(e: number): string {
  if (e > 7.2) return 'high';
  if (e > 6.5) return 'mid';
  return 'low';
}

// 安全缓解标签上色：可执行栈是危险（红），stripped 是中性提醒（黄），其余正面（绿）。
function sevClass(flag: string): string {
  if (flag.includes('危险')) return 'danger';
  if (flag.includes('剥离') || flag.includes('stripped')) return 'warn';
  return 'success';
}

const filteredExported = computed(() => {
  const q = symFilter.value.trim().toLowerCase();
  const list = props.report.symbol_summary.exported;
  return q ? list.filter((s) => s.toLowerCase().includes(q)) : list;
});

const filteredImported = computed(() => {
  const q = symFilter.value.trim().toLowerCase();
  const list = props.report.symbol_summary.imported;
  return q ? list.filter((s) => s.toLowerCase().includes(q)) : list;
});
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
.sym-cols {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: 12px;
}
.sym-head {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 8px;
  color: var(--text-secondary);
  font-size: 11px;
}
.sym-col .func-list {
  max-height: 220px;
  overflow: auto;
  padding: 8px 12px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
}
</style>
