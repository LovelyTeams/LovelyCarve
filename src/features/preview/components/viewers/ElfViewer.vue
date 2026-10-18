<template>
  <div class="report-viewer elf-viewer">
    <template v-if="report">
      <!-- 概览 -->
      <section class="card overview">
        <div class="title-row">
          <div class="title-icon"><Cpu :size="22" /></div>
          <div class="title-text">
            <h3 class="model-name">{{ report.elf_type }}</h3>
            <div class="chips">
              <span class="chip accent">{{ report.format }}</span>
              <span class="chip">{{ report.machine_label }}</span>
              <span class="chip">{{ report.endianness }}</span>
              <span class="chip">{{ report.os_abi_label }}</span>
              <span v-for="f in report.security_flags" :key="f" class="chip" :class="sevClass(f)">{{ f }}</span>
              <span v-if="report.truncated" class="chip warn">{{ tr("读取已截断") }}</span>
            </div>
          </div>
        </div>

        <div class="tiles">
          <div class="tile"><span class="tile-num">{{ report.sections.length }}</span><span class="tile-label">{{ tr("节") }}</span></div>
          <div class="tile"><span class="tile-num">{{ report.program_headers.length }}</span><span class="tile-label">{{ tr("段") }}</span></div>
          <div class="tile"><span class="tile-num">{{ report.needed.length }}</span><span class="tile-label">{{ tr("依赖库") }}</span></div>
          <div class="tile"><span class="tile-num">{{ formatBytes(report.file_size) }}</span><span class="tile-label">{{ tr("文件大小") }}</span></div>
        </div>

        <div class="detail-grid">
          <div><span class="dk">{{ tr("入口") }}</span><span class="dv mono">{{ report.entry }}</span></div>
          <div><span class="dk">ABI</span><span class="dv mono">{{ report.os_abi_label }}</span></div>
          <div v-if="report.interpreter" class="wide"><span class="dk">{{ tr("解释器") }}</span><span class="dv mono">{{ report.interpreter }}</span></div>
          <div v-if="report.soname" class="wide"><span class="dk">SONAME</span><span class="dv mono">{{ report.soname }}</span></div>
          <div v-if="report.build_id" class="wide"><span class="dk">Build ID</span><span class="dv mono">{{ report.build_id }}</span></div>
        </div>
      </section>

      <!-- 依赖库 -->
      <section v-if="report.needed.length" class="card">
        <div class="card-head"><h4 class="card-title">{{ tr("依赖库（DT_NEEDED）") }} <span class="count-badge mono">{{ report.needed.length }}</span></h4></div>
        <div class="tag-list mono">
          <span v-for="(lib, i) in report.needed" :key="i" class="tag">{{ lib }}</span>
        </div>
      </section>

      <!-- 运行时库搜索路径 -->
      <section v-if="report.rpaths.length || report.runpaths.length" class="card">
        <div class="card-head"><h4 class="card-title">{{ tr("运行时库搜索路径") }}</h4></div>
        <div class="detail-grid">
          <div v-for="(p, i) in report.runpaths" :key="`run-${i}`" class="wide"><span class="dk">RUNPATH</span><span class="dv mono">{{ p }}</span></div>
          <div v-for="(p, i) in report.rpaths" :key="`rp-${i}`" class="wide"><span class="dk">RPATH</span><span class="dv mono">{{ p }}</span></div>
        </div>
      </section>

      <!-- 程序头（段） -->
      <section v-if="report.program_headers.length" class="card">
        <div class="card-head"><h4 class="card-title">{{ tr("程序头（段）") }} <span class="count-badge mono">{{ report.program_headers.length }}</span></h4></div>
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
      <section v-if="report.sections.length" class="card">
        <div class="card-head"><h4 class="card-title">{{ tr("节") }} <span class="count-badge mono">{{ report.sections.length }}</span></h4></div>
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
      <section class="card">
        <div class="card-head">
          <h4 class="card-title">{{ tr("符号摘要") }}</h4>
          <div v-if="report.symbol_summary.exported.length || report.symbol_summary.imported.length" class="filter">
            <Search :size="12" />
            <input v-model="symFilter" type="text" :placeholder="tr('过滤符号')" spellcheck="false" />
          </div>
        </div>
        <div class="detail-grid">
          <div><span class="dk">{{ tr("动态符号") }}</span><span class="dv mono">{{ report.symbol_summary.dynsym_count }}</span></div>
          <div><span class="dk">{{ tr("调试符号") }}</span><span class="dv mono">{{ report.symbol_summary.symtab_count }}{{ report.symbol_summary.stripped ? tr(" · 已剥离") : '' }}</span></div>
        </div>
        <div class="sym-cols">
          <div v-if="report.symbol_summary.exported.length" class="sym-col">
            <div class="sym-head mono">{{ tr("导出符号") }} {{ report.symbol_summary.exported.length }}{{ report.symbol_summary.exported_truncated ? '+' : '' }}</div>
            <div class="func-list mono"><span v-for="(s, i) in filteredExported" :key="i" class="func">{{ s }}</span></div>
          </div>
          <div v-if="report.symbol_summary.imported.length" class="sym-col">
            <div class="sym-head mono">{{ tr("导入符号") }} {{ report.symbol_summary.imported.length }}{{ report.symbol_summary.imported_truncated ? '+' : '' }}</div>
            <div class="func-list mono"><span v-for="(s, i) in filteredImported" :key="i" class="func">{{ s }}</span></div>
          </div>
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

// ELF 文件的「原类型视图」：由 BinaryViewer 传入已解析的 ElfReport，按取证视角
// 渲染（机器/ABI、安全缓解、解释器/soname/依赖、build-id、段/节熵、符号摘要）。
// 视觉与 PeViewer 一致。
import { computed, ref } from 'vue';
import { AlertTriangle, Cpu, Search } from 'lucide-vue-next';
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
  if (flag.includes('危险')) return 'critical';
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
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: 12px;
  margin-top: 12px;
}
.sym-head {
  font-size: 11px;
  color: var(--text-secondary);
  margin-bottom: 6px;
}
.func-list {
  padding: 0;
  max-height: 220px;
  overflow: auto;
}
</style>
