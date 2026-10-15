<template>
  <section v-if="backup" class="ui-card ios-card" aria-labelledby="ios-backup-title">
    <header class="ui-card-header">
      <h3 id="ios-backup-title" class="ui-card-title"><Smartphone :size="16" />{{ tr("iOS 备份") }}</h3>
      <span class="ui-badge sm">{{ backup.locked ? tr("待解锁") : backup.observed_encrypted ? tr("已解锁") : tr("未加密") }}</span>
      <button v-if="!backup.locked" class="ui-btn sm" :disabled="!!busy" @click="run('audit')"><ShieldCheck :size="14" />{{ tr("检查备份") }}</button>
      <button v-if="findings" class="ui-btn sm" @click="exportFindings"><Download :size="14" />{{ tr("导出 JSON") }}</button>
    </header>
    <div class="ios-body">
      <dl class="metadata">
        <template v-for="field in fields" :key="field.label"><dt>{{ field.label }}</dt><dd>{{ field.value ?? tr("未记录") }}</dd></template>
      </dl>
      <p v-if="backup.unreadable_rows" class="ui-alert warn">{{ tr("清单有") }} {{ backup.unreadable_rows }} {{ tr("条记录无法识别，当前文件树并不完整；请结合原始 Manifest.db 复核。") }}</p>
      <p v-if="backup.unresolved_sizes" class="ui-alert warn">{{ backup.unresolved_sizes }} {{ tr("个文件未记录长度且无法读取数据，列表以 0 显示；请结合原始清单和数据文件复核。") }}</p>
      <form v-if="backup.locked" class="unlock-form" @submit.prevent="run('unlock')">
        <label :for="passwordId">{{ tr("备份密码") }}</label>
        <div class="unlock-controls">
          <input :id="passwordId" v-model="password" class="ui-input" type="password" autocomplete="off" maxlength="4096" :disabled="!!busy" :aria-invalid="error ? 'true' : undefined" :aria-describedby="error ? errorId : undefined" />
          <button class="ui-btn primary" type="submit" :disabled="!!busy">{{ tr("解锁并打开") }}</button>
        </div>
        <p class="hint">{{ tr("解锁后打开只读会话，关闭源证据或退出应用后需重新输入密码。") }}</p>
      </form>
      <div v-if="busy" class="operation" role="status"><Loader2 :size="16" class="spin" />{{ busy === 'unlock' ? tr("正在派生密钥并读取清单…") : tr("正在检查清单、数据文件与备份状态…") }}<button class="ui-btn sm" :disabled="cancelling" @click="cancel">{{ cancelling ? tr("正在终止…") : tr("终止") }}</button></div>
      <p v-if="busy === 'unlock'" class="hint">{{ tr("密钥派生可能需要较长时间，终止会在当前派生阶段结束后生效。") }}</p>
      <p v-if="error" :id="errorId" :class="backup.locked ? 'ui-field-error' : 'ui-alert danger'" role="alert">{{ error }}</p>
      <template v-if="findings">
        <p class="hint">{{ tr("共") }} {{ findings.length }} {{ tr("项观察。应用域缺失仅说明备份未包含该域，原因需结合其他证据判断；原始观察与字段保留上游表述。") }}</p>
        <div class="findings">
          <details v-for="(finding, index) in findings" :key="`${finding.code}:${index}`" class="ui-disclosure">
            <summary><span class="ui-badge sm" :class="severityTone(finding.severity)">{{ severityLabel(finding.severity) }}</span><span>{{ findingTitle(finding.code) }}</span></summary>
            <div class="finding-details">
              <p class="mono">{{ finding.code }}</p><p>{{ finding.note }}</p>
              <dl class="metadata"><template v-for="(evidence, evidenceIndex) in finding.evidence" :key="evidenceIndex"><dt>{{ evidence.field }}</dt><dd>{{ evidence.value }}</dd></template></dl>
              <p class="hint">{{ finding.source.analyzer }} {{ finding.source.version }} · {{ finding.source.scope }}</p>
            </div>
          </details>
        </div>
      </template>
    </div>
  </section>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, useId } from 'vue';
import { Download, Loader2, ShieldCheck, Smartphone } from 'lucide-vue-next';
import type { ImageInfo } from '../../../lib/ipc/image';
import type { IosBackupFinding } from '../../../lib/ipc/iosBackup';
import { useIosBackup } from '../composables/useIosBackup';
import { saveTextWithDialog } from '../../../composables/export';
import { buildJsonExport } from '../../../utils/exportFormat';

const props = defineProps<{ info: ImageInfo }>();
const emit = defineEmits<{ 'open-image': [path: string] }>();
const { password, error, busy, cancelling, findings, run, cancel } = useIosBackup(() => props.info, path => emit('open-image', path));
const passwordId = useId();
const errorId = useId();
const backup = computed(() => props.info.ios_backup);
const fields = computed(() => [
  { get label() { return tr("设备"); }, value: backup.value?.device_name },
  { get label() { return tr("型号"); }, value: backup.value?.product_type },
  { get label() { return tr("iOS 版本"); }, value: backup.value?.product_version },
  { get label() { return tr("序列号"); }, value: backup.value?.serial_number },
  { get label() { return tr("设备标识"); }, value: backup.value?.unique_identifier },
  { get label() { return tr("备份状态"); }, value: backup.value?.snapshot_state },
  { get label() { return tr("清单记录"); }, value: backup.value?.file_count?.toLocaleString() },
]);
const TITLES: Record<string, string> = {
  get 'IOS-BACKUP-ENCRYPTED'() { return tr("备份加密状态"); }, get 'IOS-BACKUP-UNENCRYPTED'() { return tr("未加密备份"); },
  get 'IOS-BACKUP-BLOB-MISSING'() { return tr("清单引用的数据文件缺失"); }, get 'IOS-BACKUP-BLOB-ORPHAN'() { return tr("未被清单引用的数据文件"); },
  get 'IOS-BACKUP-FILEID-MISMATCH'() { return tr("文件标识与路径哈希不一致"); }, get 'IOS-BACKUP-DOMAIN-ABSENT'() { return tr("预期应用域未包含在备份中"); },
  get 'IOS-BACKUP-NOT-FINISHED'() { return tr("备份未记录为完成"); }, get 'IOS-BACKUP-NO-FILE-ROWS'() { return tr("清单没有文件记录"); },
  get 'IOS-BACKUP-ENCRYPTION-STATE-CONTRADICTION'() { return tr("加密声明与清单内容不一致"); },
};
function findingTitle(code: string) { return TITLES[code] ?? code; }
function severityLabel(severity: IosBackupFinding['severity']) {
  return severity ? ({ Info: tr("信息"), Low: tr("低"), Medium: tr("中"), High: tr("高"), Critical: tr("严重") })[severity] : tr("未评级");
}
function severityTone(severity: IosBackupFinding['severity']) {
  return severity === 'Critical' || severity === 'High' ? 'danger' : severity === 'Medium' || severity === 'Low' ? 'warn' : 'info';
}
async function exportFindings() {
  if (!findings.value) return;
  await saveTextWithDialog({
    defaultPath: 'ios-backup-audit.json',
    title: tr("导出 iOS 备份检查结果"),
    filters: [{ name: 'JSON', extensions: ['json'] }],
    content: buildJsonExport(tr("iOS 备份检查"), findings.value, { image_id: props.info.id, evidence_path: props.info.path, backup: backup.value }),
    successLabel: tr("iOS 备份检查结果"),
  });
}
</script>

<style scoped>
.ios-body { display: grid; gap: 16px; padding: 16px; }
.metadata { display: grid; grid-template-columns: 100px minmax(0, 1fr); gap: 8px 12px; margin: 0; font-size: 12px; }
dt { color: var(--text-secondary); }
dd { margin: 0; overflow-wrap: anywhere; }
.unlock-form { display: grid; gap: 8px; font-size: 12px; }
.unlock-controls, .operation { display: flex; align-items: center; gap: 8px; }
.unlock-controls input { min-width: 0; flex: 1; }
.operation { font-size: 12px; }
.operation button { margin-left: auto; }
.hint { margin: 0; font-size: 11px; line-height: 1.6; color: var(--text-secondary); }
.findings { display: grid; gap: 8px; max-height: 480px; overflow: auto; }
summary { display: flex; align-items: center; gap: 8px; }
.finding-details { display: grid; gap: 8px; padding: 12px; font-size: 12px; }
.finding-details p { margin: 0; overflow-wrap: anywhere; }
.mono { font-family: var(--font-mono); }
</style>
