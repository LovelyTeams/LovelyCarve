<template>
  <Teleport to="body">
    <Transition name="ui-modal">
      <div v-if="inspection" class="ui-modal-overlay" @click.self="close">
        <div ref="dialogEl" class="ui-modal install-modal" role="dialog" aria-modal="true"
          :aria-labelledby="titleId" tabindex="-1" @keydown.esc.stop="close">
          <header class="ui-modal-header">
            <span class="ui-modal-icon"><PackagePlus :size="18" aria-hidden="true" /></span>
            <div class="ui-modal-heading">
              <h2 :id="titleId" class="ui-modal-title">{{ tr("安装插件包") }}</h2>
              <p class="ui-modal-subtitle" :title="name">{{ name }}</p>
            </div>
            <button class="ui-icon-btn" type="button" :title="tr('关闭')" :aria-label="tr('关闭')" :disabled="installing" @click="close"><X :size="16" /></button>
          </header>
          <div class="ui-modal-body install-body">
            <div v-if="inspection.blocked" class="ui-alert danger" role="alert">
              <CircleAlert :size="16" aria-hidden="true" />
              <div class="alert-text"><strong>{{ blockTitle(inspection.blocked.reason, tr) }}</strong><span>{{ inspection.blocked.message }}</span></div>
            </div>
            <div v-if="failure" class="ui-alert danger" role="alert">
              <CircleAlert :size="16" aria-hidden="true" />
              <div class="alert-text"><strong>{{ failure.title }}</strong><span>{{ failure.detail }}</span></div>
            </div>
            <div class="install-badges">
              <span class="ui-badge" :class="changeIsWarning(inspection.change) ? 'warn' : 'info'">{{ changeLabel(inspection.change, inspection.installed?.version ?? null, tr) }}</span>
              <span v-for="permission in inspection.permissions" :key="permission" class="ui-badge warn">{{ permissionLabel(permission, tr) }}</span>
            </div>
            <FileDetailFields :fields="fields" />
          </div>
          <footer class="ui-modal-footer">
            <button class="ui-btn ghost" type="button" :disabled="installing" @click="close">{{ tr("取消") }}</button>
            <button class="ui-btn primary" type="button" :disabled="installing || inspection.blocked !== null" @click="emit('confirm')">
              <Loader2 v-if="installing" :size="14" class="spin" /><PackagePlus v-else :size="14" aria-hidden="true" />{{ tr("安装") }}
            </button>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { tr, useLocale } from '../../../composables/useLocale';

/** Confirms installing an inspected package: what it is, how it relates to
 *  the installed version, what it declares and the digests that install. */
import { computed, ref, useId } from 'vue';
import { CircleAlert, Loader2, PackagePlus, X } from 'lucide-vue-next';
import FileDetailFields, { type FileDetailField } from '../../../components/layout/FileDetailFields.vue';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import { joinList } from '../../../i18n/translate';
import type { PackageInspection } from '../../../lib/ipc/plugins';
import { pluginText, pluginTextKey } from '../../../lib/models/pluginText';
import { formatBytes } from '../../../utils/format';
import { blockTitle, changeIsWarning, changeLabel, permissionLabel, type PluginFailure } from '../model/pluginPackages';

const props = defineProps<{
  inspection: PackageInspection | null;
  /** The market source it comes from; a local file shows its path. */
  source?: string | null;
  installing?: boolean;
  failure?: PluginFailure | null;
}>();
const emit = defineEmits<{ confirm: []; close: [] }>();

const { locale } = useLocale();
const titleId = useId();
const dialogEl = ref<HTMLElement | null>(null);
useDialogFocus(() => props.inspection !== null, dialogEl);

function text(key: string, fallback: string): string {
  return pluginText(props.inspection?.locales, locale.value, key, fallback);
}

const name = computed(() => props.inspection ? text(pluginTextKey.plugin(), props.inspection.name) : '');

const fields = computed<FileDetailField[]>(() => {
  const item = props.inspection;
  if (!item) return [];
  const description = item.description ? text(pluginTextKey.description(), item.description) : null;
  const rows: FileDetailField[] = [
    { label: tr("标识"), value: item.pluginId, mono: true },
    { label: tr("版本"), value: item.installed ? `${item.installed.version} → ${item.version}` : item.version, mono: true, warning: changeIsWarning(item.change) },
  ];
  if (description) rows.push({ label: tr("描述"), value: description });
  if (item.publisher) rows.push({ label: tr("发布者"), value: item.publisher });
  if (item.license) rows.push({ label: tr("许可证"), value: item.license, mono: true });
  rows.push(
    { label: tr("权限"), value: item.permissions.length ? joinList(item.permissions.map(permission => permissionLabel(permission, tr)), tr) : tr("无需额外权限") },
    { label: tr("分析器"), value: item.analyzers.length ? joinList(item.analyzers.map(analyzer => text(pluginTextKey.analyzer(analyzer.id), analyzer.name)), tr) : tr("暂无") },
  );
  if (item.filesystems.length) rows.push({ label: tr("文件系统"), value: item.filesystems.join(' '), mono: true });
  rows.push(
    { label: tr("记录类型"), value: item.recordTypes },
    { label: tr("内存上限"), value: formatBytes(item.memoryLimit) },
    { label: 'SDK', value: item.sdk ?? tr("暂无"), mono: true },
    { label: tr("来源"), value: props.source ?? item.archive, mono: true },
    { label: tr("大小"), value: formatBytes(item.size) },
    { label: 'SHA-256', value: item.sha256, mono: true, copyable: true },
    { label: tr("库 SHA-256"), value: item.librarySha256 ?? '—', mono: true, copyable: item.librarySha256 !== null },
  );
  return rows;
});

function close() {
  if (!props.installing) emit('close');
}
</script>

<style scoped>
.install-modal { width: min(560px, 100%); }
.install-body { display: flex; flex-direction: column; gap: 12px; }
.install-badges { display: flex; flex-wrap: wrap; gap: 6px; }
.alert-text { display: flex; flex: 1; min-width: 0; flex-direction: column; gap: 2px; overflow-wrap: anywhere; }
.alert-text span { font-size: 11px; }
</style>
