<template>
  <div class="report-viewer kdbx-info">
    <header class="report-head">
      <div class="report-icon"><DatabaseIcon :size="20" /></div>
      <div class="report-title">
        <h3 class="report-name">{{ database.meta.database_name || tr("KeePass 数据库") }}</h3>
        <div class="report-badges">
          <span class="ui-badge sm mono">KDBX {{ database.header.version }}</span>
          <span class="ui-badge sm mono">{{ database.header.cipher }}</span>
          <span class="ui-badge sm mono">{{ database.header.kdf.name }}</span>
          <span v-if="database.meta.generator" class="ui-badge sm">{{ database.meta.generator }}</span>
        </div>
      </div>
    </header>

    <dl class="report-stats">
      <div class="report-stat"><dt>{{ tr("条目") }}</dt><dd>{{ database.counts.entries.toLocaleString() }}</dd></div>
      <div class="report-stat"><dt>{{ tr("群组") }}</dt><dd>{{ database.counts.groups.toLocaleString() }}</dd></div>
      <div class="report-stat"><dt>{{ tr("历史版本") }}</dt><dd>{{ database.counts.history.toLocaleString() }}</dd></div>
      <div class="report-stat"><dt>{{ tr("附件") }}</dt><dd>{{ database.counts.attachments.toLocaleString() }}</dd></div>
      <div class="report-stat"><dt>{{ tr("已删除对象") }}</dt><dd>{{ database.deleted_objects.length.toLocaleString() }}</dd></div>
    </dl>

    <section v-if="warnings.length" class="ui-alert warn report-warnings" role="note">
      <AlertTriangle :size="14" />
      <div class="warn-body">
        <p class="warn-title">{{ tr("读取警告") }}</p>
        <p v-for="(warning, index) in warnings" :key="index" class="warn-line mono">{{ warning }}</p>
      </div>
    </section>

    <section class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("数据库") }}</h4></div>
      <KdbxDetailList :rows="meta" />
    </section>

    <section class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("解密") }}</h4></div>
      <KdbxDetailList :rows="keyFields" />
    </section>

    <section class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("外部头部") }}</h4></div>
      <KdbxDetailList :rows="header" />
    </section>

    <section class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("头部字段") }} <span class="ui-count">{{ database.header.fields.length }}</span></h4></div>
      <div class="table-scroll">
        <table class="data-table g-table">
          <thead><tr><th class="ralign">ID</th><th>{{ tr("名称") }}</th><th class="ralign">{{ tr("偏移") }}</th><th class="ralign">{{ tr("长度") }}</th></tr></thead>
          <tbody>
            <tr v-for="field in database.header.fields" :key="field.offset">
              <td class="mono ralign">{{ field.id }}</td>
              <td class="mono">{{ field.name ?? '—' }}</td>
              <td class="mono ralign">0x{{ field.offset.toString(16).toUpperCase() }}</td>
              <td class="mono ralign">{{ field.length.toLocaleString() }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <section v-if="database.binaries.length" class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("附件池") }} <span class="ui-count">{{ database.binaries.length }}</span></h4></div>
      <div class="table-scroll">
        <table class="data-table g-table">
          <thead><tr><th class="ralign">ID</th><th class="ralign">{{ tr("大小") }}</th><th>SHA-256</th><th>{{ tr("状态") }}</th></tr></thead>
          <tbody>
            <tr v-for="binary in database.binaries" :key="binary.id">
              <td class="mono ralign">{{ binary.id }}</td>
              <td class="mono ralign">{{ formatBytes(binary.size) }}</td>
              <td class="mono hash" :title="binary.sha256">{{ binary.sha256 }}</td>
              <td>
                <span v-if="binary.unreadable" class="ui-badge sm danger" :title="binary.unreadable">{{ tr("无法读取") }}</span>
                <span v-if="binary.protected" class="ui-badge sm">{{ tr("受保护") }}</span>
                <span v-if="binary.compressed" class="ui-badge sm">GZip</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <section v-if="customData.length" class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("自定义数据") }} <span class="ui-count">{{ customData.length }}</span></h4></div>
      <div class="table-scroll">
        <table class="data-table g-table">
          <thead><tr><th>{{ tr("来源") }}</th><th>{{ tr("键") }}</th><th>{{ tr("值") }}</th><th>{{ tr("修改时间") }}</th></tr></thead>
          <tbody>
            <tr v-for="(item, index) in customData" :key="index">
              <td>{{ item.source }}</td>
              <td class="mono">{{ item.key }}</td>
              <td class="mono value-cell" :title="item.value">{{ item.value }}</td>
              <td class="mono" :title="kdbxTimeText(item.last_modification).title">{{ kdbxTimeText(item.last_modification).text }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <section v-if="database.meta.custom_icons.length" class="section">
      <div class="section-head"><h4 class="section-title">{{ tr("自定义图标") }} <span class="ui-count">{{ database.meta.custom_icons.length }}</span></h4></div>
      <div class="table-scroll">
        <table class="data-table g-table">
          <thead><tr><th>UUID</th><th>{{ tr("名称") }}</th><th class="ralign">{{ tr("大小") }}</th><th>{{ tr("修改时间") }}</th></tr></thead>
          <tbody>
            <tr v-for="(icon, index) in database.meta.custom_icons" :key="index">
              <td class="mono">{{ icon.uuid ?? '—' }}</td>
              <td>{{ icon.name || '—' }}</td>
              <td class="mono ralign">{{ formatBytes(icon.size) }}</td>
              <td class="mono" :title="kdbxTimeText(icon.last_modification).title">{{ kdbxTimeText(icon.last_modification).text }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { computed } from 'vue';
import { AlertTriangle, Database as DatabaseIcon } from 'lucide-vue-next';
import type { KdbxDatabase } from '../../../../lib/ipc/kdbx';
import { formatBytes } from '../../../../utils/format';
import { headerDetails, kdbxTimeText, keyDetails, metaDetails } from '../../model/kdbx';
import KdbxDetailList from './KdbxDetailList.vue';

const props = defineProps<{
  database: KdbxDatabase;
  groupName: (uuid: string | null) => string | null;
}>();

const meta = computed(() => metaDetails(props.database, props.groupName, tr));
const keyFields = computed(() => keyDetails(props.database, tr));
const header = computed(() => headerDetails(props.database.header, tr));
const warnings = computed(() => [
  ...(props.database.header_hash === 'mismatch' ? [tr('Meta/HeaderHash 与头部不符')] : []),
  ...props.database.header.warnings,
  ...props.database.warnings,
]);

/** Database-level custom data, then the public custom data of the header. */
const customData = computed(() => [
  ...props.database.meta.custom_data.map(item => ({ ...item, source: tr('数据库') })),
  ...props.database.header.public_custom_data.map(item => ({
    key: item.key, value: item.value, last_modification: null, source: tr('外部头部'),
  })),
]);
</script>

<style scoped src="../../../../styles/data-table.css"></style>
<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
.kdbx-info {
  min-height: 0;
}

.hash,
.value-cell {
  max-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  width: 60%;
}
</style>
