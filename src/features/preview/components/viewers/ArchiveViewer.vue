<template>
  <div class="report-viewer archive-viewer">
    <template v-if="report">
      <!-- 概览 -->
      <section class="card overview">
        <div class="title-row">
          <div class="title-icon"><Library :size="22" /></div>
          <div class="title-text">
            <h3 class="model-name">{{ report.format }}</h3>
            <div class="chips">
              <span class="chip accent">{{ report.member_count }} {{ tr("成员") }}</span>
              <span class="chip">{{ report.total_symbols }} {{ tr("符号") }}</span>
              <span v-if="report.truncated" class="chip warn">{{ tr("读取已截断") }}</span>
            </div>
          </div>
        </div>

        <div class="tiles">
          <div class="tile"><span class="tile-num">{{ report.member_count }}</span><span class="tile-label">{{ tr("成员") }}</span></div>
          <div class="tile"><span class="tile-num">{{ report.total_symbols }}</span><span class="tile-label">{{ tr("导出符号") }}</span></div>
          <div class="tile"><span class="tile-num">{{ formatBytes(report.file_size) }}</span><span class="tile-label">{{ tr("文件大小") }}</span></div>
        </div>
      </section>

      <!-- 成员 -->
      <section class="card">
        <div class="card-head">
          <h4 class="card-title">{{ tr("成员") }} <span class="count-badge mono">{{ report.member_count }}</span></h4>
          <div class="filter">
            <Search :size="12" />
            <input v-model="memberFilter" type="text" :placeholder="tr('过滤成员名')" spellcheck="false" />
          </div>
        </div>
        <div class="table-scroll">
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

// Unix ar 归档（.a 静态库 / 导入库）的「原类型视图」：由 BinaryViewer 传入已解析
// 的 ArchiveReport，列出成员（名称 / 格式 / 大小 / 数据偏移 / 导出符号数）与符号
// 总数。视觉与 PeViewer 一致。
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
.table-scroll {
  max-height: 520px;
}
</style>
