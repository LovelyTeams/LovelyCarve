<template>
  <div class="report-viewer macho-viewer">
    <template v-if="report">
      <!-- 概览 -->
      <header class="report-head">
        <div class="report-icon"><Binary :size="20" /></div>
        <div class="report-title">
          <h3 class="report-name">{{ report.filetype_label }}</h3>
          <div class="report-badges">
            <span class="ui-badge sm mono">{{ report.format }}</span>
            <span class="ui-badge sm mono">{{ report.cpu_label }}</span>
            <span v-if="report.is_fat" class="ui-badge sm info">{{ tr("通用二进制 ·") }} {{ report.arches.length }} {{ tr("架构") }}</span>
            <span v-for="f in report.flags" :key="f" class="ui-badge sm mono">{{ f }}</span>
            <span v-if="report.code_signature" class="ui-badge sm success"><ShieldCheck :size="11" />{{ tr("代码签名") }}</span>
            <span v-if="report.encrypted" class="ui-badge sm danger"><AlertTriangle :size="11" />{{ tr("已加密") }}</span>
            <span v-if="report.truncated" class="ui-badge sm warn">{{ tr("读取已截断") }}</span>
          </div>
        </div>
      </header>

      <dl class="report-stats">
        <div class="report-stat"><dt>{{ tr("段") }}</dt><dd>{{ report.segments.length }}</dd></div>
        <div class="report-stat"><dt>{{ tr("加载命令") }}</dt><dd>{{ lcTotal }}</dd></div>
        <div class="report-stat"><dt>{{ tr("依赖 dylib") }}</dt><dd>{{ report.libs.length }}</dd></div>
        <div class="report-stat"><dt>{{ tr("文件大小") }}</dt><dd>{{ formatBytes(report.file_size) }}</dd></div>
      </dl>

      <dl class="report-fields">
        <div><dt>{{ tr("入口") }}</dt><dd>{{ report.entry }}</dd></div>
        <div v-if="report.min_os"><dt>{{ tr("最低系统") }}</dt><dd>{{ report.min_os }}</dd></div>
        <div v-if="report.uuid" class="wide"><dt>UUID</dt><dd>{{ report.uuid }}</dd></div>
      </dl>

      <!-- 解析警告 -->
      <section v-if="report.parse_warnings.length" class="ui-alert warn report-warnings" role="note">
        <AlertTriangle :size="14" />
        <div class="warn-body">
          <p class="warn-title">{{ tr("解析警告") }}</p>
          <p v-for="(w, i) in report.parse_warnings" :key="i" class="warn-line mono">{{ w }}</p>
        </div>
      </section>

      <!-- 架构（Fat） -->
      <section v-if="report.is_fat && report.arches.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("架构") }} <span class="ui-count">{{ report.arches.length }}</span></h4></div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead><tr><th>{{ tr("架构") }}</th><th>{{ tr("偏移") }}</th><th>{{ tr("大小") }}</th></tr></thead>
            <tbody>
              <tr v-for="(a, i) in report.arches" :key="i">
                <td class="mono strong">{{ a.cpu_label }}</td>
                <td class="mono">{{ hex(a.offset) }}</td>
                <td class="mono">{{ formatBytes(a.size) }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <!-- 依赖 dylib -->
      <section v-if="report.libs.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("依赖 dylib") }} <span class="ui-count">{{ report.libs.length }}</span></h4></div>
        <div class="tag-list">
          <span v-for="(lib, i) in report.libs" :key="i" class="tag">{{ lib }}</span>
        </div>
      </section>

      <!-- 段 + 节 -->
      <section v-if="report.segments.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("段") }} <span class="ui-count">{{ report.segments.length }}</span></h4></div>
        <div class="framed-list">
          <details v-for="(seg, i) in report.segments" :key="i" v-disclosure-motion class="ui-disclosure disclosure" :open="report.segments.length <= 4">
            <summary>
              <ChevronRight :size="13" class="ui-chevron" />
              <span class="mono strong">{{ seg.name || tr("（未命名段）") }}</span>
              <span class="ui-badge sm mono">{{ seg.initprot }}</span>
              <span class="dim mono">vm {{ seg.vmaddr }} · {{ formatBytes(seg.vmsize) }} · {{ seg.nsects }} {{ tr("节") }}</span>
            </summary>
            <div v-if="seg.sections.length" class="table-scroll segment-sections">
              <table class="data-table g-table">
                <thead><tr><th>{{ tr("节") }}</th><th>{{ tr("地址") }}</th><th>{{ tr("文件偏移") }}</th><th>{{ tr("大小") }}</th><th class="th-entropy">{{ tr("熵") }}</th></tr></thead>
                <tbody>
                  <tr v-for="(s, si) in seg.sections" :key="si" :class="{ 'row-flag': s.entropy != null && s.entropy > 7.2 }">
                    <td class="mono strong">{{ s.name }}</td>
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
            <div v-else class="empty-row">{{ tr("无节") }}</div>
          </details>
        </div>
      </section>

      <!-- 加载命令分布 -->
      <section v-if="report.load_commands.length" class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("加载命令分布") }} <span class="ui-count">{{ lcTotal }}</span></h4></div>
        <div class="table-scroll">
          <table class="data-table g-table">
            <thead><tr><th>{{ tr("命令") }}</th><th class="ralign">{{ tr("数量") }}</th></tr></thead>
            <tbody>
              <tr v-for="(lc, i) in report.load_commands" :key="i">
                <td class="mono strong">{{ lc.name }}</td>
                <td class="mono ralign">{{ lc.count }}</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <!-- 符号摘要 -->
      <section class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("符号摘要") }}</h4></div>
        <dl class="report-stats">
          <div class="report-stat"><dt>{{ tr("符号总数") }}</dt><dd>{{ report.symbol_summary.total }}{{ report.symbol_summary.truncated ? '+' : '' }}</dd></div>
          <div class="report-stat"><dt>{{ tr("已定义") }}</dt><dd>{{ report.symbol_summary.defined }}</dd></div>
          <div class="report-stat"><dt>{{ tr("未定义（导入）") }}</dt><dd>{{ report.symbol_summary.undefined }}</dd></div>
          <div class="report-stat"><dt>{{ tr("已剥离") }}</dt><dd>{{ report.symbol_summary.stripped ? tr("是") : tr("否") }}</dd></div>
        </dl>
      </section>
    </template>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { vDisclosureMotion } from '../../../../composables/uiMotionDirectives';
// Mach-O 文件的「原类型视图」：由 BinaryViewer 传入已解析的 MachoReport，按取证
// 视角渲染（CPU/类型/标志、UUID、最低系统、依赖 dylib、段/节熵、加载命令分布、
// 符号摘要、代码签名/加密）。通用二进制列出全部架构。版式与 PeViewer 一致。
import { computed } from 'vue';
import { AlertTriangle, Binary, ChevronRight, ShieldCheck } from 'lucide-vue-next';
import type { MachoReport } from '../../../../lib/ipc/binary';
import { formatBytes } from '../../../../utils/format';

const props = defineProps<{
  report: MachoReport;
}>();

const report = computed(() => props.report);
const lcTotal = computed(() => props.report.load_commands.reduce((acc, c) => acc + c.count, 0));

function hex(n: number): string {
  return `0x${n.toString(16)}`;
}

function entropyClass(e: number): string {
  if (e > 7.2) return 'high';
  if (e > 6.5) return 'mid';
  return 'low';
}
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
.segment-sections {
  margin: 0 0 12px 20px;
}
</style>
