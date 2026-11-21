<template>
  <div class="report-viewer archive-viewer">
    <template v-if="report">
      <!-- 概览 -->
      <header class="report-head">
        <div class="report-icon"><Library :size="20" /></div>
        <div class="report-title">
          <h3 class="report-name">{{ report.format }}</h3>
          <div class="report-badges">
            <span v-if="report.truncated" class="ui-badge sm warn">{{ tr("读取已截断") }}</span>
          </div>
        </div>
      </header>

      <dl class="report-stats">
        <div class="report-stat"><dt>{{ tr("成员") }}</dt><dd>{{ report.member_count }}</dd></div>
        <div class="report-stat"><dt>{{ tr("导出符号") }}</dt><dd>{{ report.total_symbols }}</dd></div>
        <div class="report-stat"><dt>{{ tr("文件大小") }}</dt><dd>{{ formatBytes(report.file_size) }}</dd></div>
      </dl>

      <!-- 解析警告 -->
      <section v-if="report.parse_warnings.length" class="ui-alert warn report-warnings" role="note">
        <AlertTriangle :size="14" />
        <div class="warn-body">
          <p class="warn-title">{{ tr("解析警告") }}</p>
          <p v-for="(w, i) in report.parse_warnings" :key="i" class="warn-line mono">{{ w }}</p>
        </div>
      </section>

      <!-- 成员 -->
      <section class="section">
        <div class="section-head">
          <h4 class="section-title">{{ tr("成员") }} <span class="ui-count">{{ report.member_count }}</span></h4>
          <label class="ui-search section-filter">
            <Search :size="13" aria-hidden="true" />
            <input v-model="memberFilter" type="text" :placeholder="tr('过滤成员名')" :aria-label="tr('过滤成员名')" spellcheck="false" />
          </label>
        </div>
        <div class="table-scroll members">
          <table class="data-table g-table">
            <thead><tr><th>{{ tr("名称") }}</th><th>{{ tr("格式") }}</th><th>{{ tr("大小") }}</th><th>{{ tr("数据偏移") }}</th><th class="ralign">{{ tr("符号") }}</th></tr></thead>
            <tbody>
              <tr v-for="(m, i) in filteredMembers" :key="i">
                <td class="mono strong">{{ m.name }}</td>
                <td class="mono">{{ m.member_format }}</td>
                <td class="mono">{{ formatBytes(m.size) }}</td>
                <td class="mono">{{ m.data_offset }}</td>
                <td class="mono ralign">{{ m.symbol_count }}</td>
              </tr>
              <tr v-if="filteredMembers.length === 0">
                <td colspan="5" class="empty-row">{{ tr("无匹配项") }}</td>
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

// Unix ar 归档（.a 静态库 / 导入库）的「原类型视图」：由 BinaryViewer 传入已解析
// 的 ArchiveReport，列出成员（名称 / 格式 / 大小 / 数据偏移 / 导出符号数）与符号
// 总数。版式与 PeViewer 一致。
import { computed, ref } from 'vue';
import { AlertTriangle, Library, Search } from 'lucide-vue-next';
import type { ArchiveReport } from '../../../../lib/ipc/binary';
import { formatBytes } from '../../../../utils/format';

const props = defineProps<{
  report: ArchiveReport;
}>();

const report = computed(() => props.report);
const memberFilter = ref('');

const filteredMembers = computed(() => {
  const q = memberFilter.value.trim().toLowerCase();
  if (!q) return props.report.members;
  return props.report.members.filter((m) => m.name.toLowerCase().includes(q));
});
</script>

<style scoped src="../../../../styles/data-table.css"></style>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
.members {
  max-height: 520px;
}
</style>
