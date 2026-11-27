<template>
  <Teleport to="body">
    <Transition name="ui-modal">
      <div v-show="open" class="ui-modal-overlay" @click.self="emit('close')">
        <div
          ref="dialogEl"
          class="ui-modal unlock-modal"
          role="dialog"
          aria-modal="true"
          aria-labelledby="encrypted-unlock-title"
          tabindex="-1"
          @keydown.esc.stop="event => { if (!event.isComposing) emit('close'); }"
        >
          <header class="ui-modal-header">
            <span class="ui-modal-icon"><LockKeyhole :size="18" /></span>
            <div class="ui-modal-heading">
              <h2 id="encrypted-unlock-title" class="ui-modal-title">{{ partitionId === null ? tr("解锁加密卷") : tr("解锁分区 {0}", { 0: partitionId }) }}</h2>
            </div>
            <button class="ui-icon-btn" type="button" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
          </header>
          <form :id="formId" class="unlock-form" @submit.prevent="unlock">
            <div class="ui-modal-body unlock-body">
              <p v-if="!schemeOptions.length" class="ui-alert" role="status">{{ tr("没有可解锁的加密区域") }}</p>
              <template v-else>
                <fieldset :class="{ fixed: partitionId !== null }">
                  <label>{{ tr("加密格式") }}
                    <UiSelect v-model="scheme" :options="schemeOptions" :disabled="!open || schemeOptions.length === 1" />
                  </label>
                  <label v-if="partitionId === null">{{ tr("加密区域") }}
                    <UiSelect v-model="region" :options="regionOptions" :disabled="!open" />
                  </label>
                </fieldset>
                <fieldset v-if="scheme === 'filevault'">
                  <label class="wide">{{ tr("外部加密上下文（可选）") }}
                    <input v-model="encryptedRootPath" class="ui-input" :placeholder="tr('EncryptedRoot.plist.wipekey 或 XML 的本地路径')" :aria-invalid="encryptedRootPath && inventoryError ? 'true' : undefined" :aria-describedby="inventoryNoteId" @input="resetInventory" />
                  </label>
                  <div class="wide"><button class="ui-btn ghost sm" type="button" :disabled="inventoryBusy" @click="loadInventory">{{ inventoryBusy ? tr("正在读取卷信息…") : tr("读取卷信息") }}</button></div>
                  <div v-if="inventoryError" :id="inventoryNoteId" class="wide ui-field-error" role="alert">{{ inventoryError }}</div>
                  <label v-if="inventory" class="wide">{{ tr("逻辑卷") }}
                    <UiSelect v-model="logicalVolumeIndex" :options="logicalVolumeOptions" :disabled="!open" />
                    <span v-if="selectedLogicalVolume && selectedLogicalVolume.conversion_status !== 'Complete'" class="ui-field-error" role="alert">{{ tr("加密尚未完成，无法解锁") }}</span>
                  </label>
                </fieldset>
                <fieldset v-if="scheme === 'apfs'">
                  <p v-if="apfsLoading" class="wide" role="status">{{ tr("正在读取卷信息…") }}</p>
                  <p v-else-if="apfsError" class="wide ui-field-error" role="alert">{{ apfsError }}</p>
                  <label v-else-if="apfsVolumeOptions.length" class="wide">{{ tr("逻辑卷") }}
                    <UiSelect v-model="apfsVolumeIndex" :options="apfsVolumeOptions" :disabled="!open" />
                  </label>
                  <p v-else class="wide ui-alert" role="status">{{ tr("没有锁定的 APFS 卷") }}</p>
                </fieldset>
                <div v-if="modes.length > 1" class="ui-seg" role="group" :aria-label="tr('解锁凭据')">
                  <button v-for="item in modes" :key="item" class="ui-seg-item" :class="{ active: mode === item }" type="button"
                    :aria-pressed="mode === item" @click="mode = item">{{ credentialModeLabel(item, tr) }}</button>
                </div>
                <fieldset v-if="scheme !== 'apfs' || apfsVolumeOptions.length">
                  <label v-if="mode === 'password'" class="wide">{{ passwordLabel }}
                    <input v-model="password" class="ui-input" type="password" autocomplete="off" spellcheck="false" maxlength="4096" />
                  </label>
                  <label v-else-if="mode === 'masterKey'" class="wide">{{ tr("主密钥（十六进制）") }}
                    <input v-model="masterKey" class="ui-input mono" type="password" autocomplete="off" maxlength="1024" :aria-invalid="keyError ? 'true' : undefined" :aria-describedby="keyError ? keyNoteId : undefined" @blur="keyTouched = true" />
                    <span v-if="keyError" :id="keyNoteId" class="ui-field-error" role="alert">{{ keyError }}</span>
                  </label>
                  <label v-else-if="mode === 'recovery'" class="wide">{{ tr("恢复密钥") }}
                    <input v-model="recoveryInput" class="ui-input mono" type="text" autocomplete="off" spellcheck="false"
                      placeholder="xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx"
                      :aria-invalid="recoveryError ? 'true' : undefined" :aria-describedby="recoveryError ? recoveryNoteId : undefined" @blur="onRecoveryBlur" />
                    <span v-if="recoveryError" :id="recoveryNoteId" class="ui-field-error" role="alert">{{ recoveryError }}</span>
                  </label>
                  <BitlockerMemoryCandidates v-else-if="mode === 'memory'" ref="memoryFields" v-model:credential="memoryCredential" class="wide"
                    :image-id="info.id" :partition-id="selectedPartitionId" :active="open" />
                  <BitlockerKeyFields v-else-if="mode === 'otherKey'" v-model:credential="keyCredential" class="wide" :active="open" />
                  <template v-if="scheme === 'veracrypt'">
                    <div class="wide keyfiles">
                      <button class="ui-btn ghost sm" type="button" :disabled="!open" @click="pickKeyfiles(false)"><FileKey :size="13" />{{ tr("添加密钥文件") }}</button>
                      <button class="ui-btn ghost sm" type="button" :disabled="!open" @click="pickKeyfiles(true)"><FolderKey :size="13" />{{ tr("添加密钥文件夹") }}</button>
                    </div>
                    <ul v-if="keyfiles.length" class="wide keyfile-list" :aria-label="tr('密钥文件')">
                      <li v-for="(path, index) in keyfiles" :key="path">
                        <span class="key-path mono" :title="path">{{ path }}</span>
                        <button class="ui-icon-btn sm" type="button" :title="tr('移除')" :aria-label="tr('移除密钥文件 {0}', { 0: path })" @click="keyfiles.splice(index, 1)"><X :size="12" /></button>
                      </li>
                    </ul>
                    <label>{{ tr("卷头") }}
                      <UiSelect v-model="veracryptHeader" :options="headerOptions" :disabled="!open" />
                    </label>
                    <label>{{ tr("密钥派生") }}
                      <UiSelect v-model="veracryptPrf" :options="prfOptions" :disabled="!open" />
                    </label>
                    <label>PIM
                      <input v-model.number="pim" class="ui-input mono" type="number" min="0" :max="pimLimit" step="1" required
                        :aria-invalid="pimError ? 'true' : undefined" :aria-describedby="pimError ? pimNoteId : undefined" />
                      <span v-if="pimError" :id="pimNoteId" class="ui-field-error" role="alert">{{ pimError }}</span>
                    </label>
                  </template>
                </fieldset>
              </template>
            </div>
            <footer class="ui-modal-footer">
              <button class="ui-btn" type="button" @click="emit('close')">{{ tr("取消") }}</button>
              <button class="ui-btn primary" type="submit" :disabled="!canUnlock">{{ tr("解锁") }}</button>
            </footer>
          </form>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, onBeforeUnmount, ref, useId, watch } from 'vue';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { FileKey, FolderKey, LockKeyhole, X } from 'lucide-vue-next';
import type { DiskPartition, ImageInfo } from '../../../lib/ipc/image';
import { apfsInspect, type ApfsEncryptionVolume, type EncryptedUnlockOptions, type EncryptionScheme, type UnlockCredential, type VeracryptHeader } from '../../../lib/ipc/encrypted';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import { startEncryptedVolumeUnlock } from '../composables/useEncryptedVolumeUnlocks';
import { useFilevaultInventory } from '../composables/useFilevaultInventory';
import { useBitlockerRecoveryKey } from '../composables/useBitlockerRecoveryKey';
import { dismissUnlockReminder } from '../composables/useFileSession';
import {
  availableSchemes, credentialModeLabel, credentialModes, initialSelection, schemeLabel, unlockRegions,
  VERACRYPT_HEADERS, VERACRYPT_PRFS, veracryptHeaderLabel, veracryptPimLimit, veracryptPrfFits, veracryptPrfLabel,
  type CredentialMode, type VeracryptPrfChoice,
} from '../model/encryptedUnlock';
import { formatBytes } from '../../../utils/format';
import UiSelect, { type UiSelectOption } from '../../../components/ui/UiSelect.vue';
import BitlockerKeyFields from './BitlockerKeyFields.vue';
import BitlockerMemoryCandidates from './BitlockerMemoryCandidates.vue';

const props = defineProps<{
  open: boolean;
  info: ImageInfo;
  /** 从分区行打开时固定该分区，从卷卡标题打开时允许选择区域。 */
  partitionId: number | null;
}>();
const emit = defineEmits<{ close: [] }>();

// ── 格式与区域 ────────────────────────────────────────────────────────────
const scheme = ref<EncryptionScheme>('luks');
const mode = ref<CredentialMode>('password');
const schemeOptions = computed<UiSelectOption<EncryptionScheme>[]>(() =>
  availableSchemes(props.info, props.partitionId).map(value => ({ value, label: schemeLabel(value) })));
const region = ref('whole');
const regions = computed(() => unlockRegions(props.info, scheme.value));
function regionLabel(partition: DiskPartition) {
  return tr("分区 {0} · {1}", { 0: partition.id, 1: partition.label || partition.kind });
}
const regionOptions = computed<UiSelectOption<string>[]>(() => regions.value.map(candidate => {
  const partition = candidate.partitionId === undefined ? undefined : props.info.partitions.find(item => item.id === candidate.partitionId);
  return partition
    ? { value: String(partition.id), get label() { return regionLabel(partition); } }
    : { value: 'whole', get label() { return tr("整个镜像 / 容器"); } };
}));
const selectedPartitionId = computed(() => props.partitionId ?? (region.value === 'whole' ? undefined : Number(region.value)));
const regionValid = computed(() => props.partitionId !== null
  ? regions.value.some(candidate => candidate.partitionId === props.partitionId)
  : regionOptions.value.some(option => option.value === region.value));

// A scheme keeps the region when it can open it, and otherwise starts from its first one.
watch(scheme, next => {
  if (props.partitionId === null && !regionOptions.value.some(option => option.value === region.value)) {
    region.value = regionOptions.value[0]?.value ?? 'whole';
  }
  if (!credentialModes(next).includes(mode.value)) mode.value = credentialModes(next)[0]!;
});

// ── 凭据 ──────────────────────────────────────────────────────────────────
const modes = computed(() => credentialModes(scheme.value));
const passwordLabel = computed(() => {
  switch (scheme.value) {
    case 'bitlocker': return tr("用户密码");
    case 'apfs':
    case 'filevault': return tr("密码 / 个人恢复密码");
    case 'luks':
    case 'veracrypt': return tr("密码");
  }
});
const password = ref('');
const masterKey = ref('');
const keyTouched = ref(false);
const keyNoteId = useId();
const normalizedKey = computed(() => masterKey.value.replace(/\s/g, ''));
const keyError = computed(() => {
  if (/[^0-9a-f]/i.test(normalizedKey.value)) return tr("主密钥只能包含十六进制字符 0–9、A–F。");
  if (scheme.value === 'filevault' && (keyTouched.value || normalizedKey.value.length > 0) && ![32, 64].includes(normalizedKey.value.length)) return tr("FileVault 密钥需要 16 或 32 字节（32 或 64 个十六进制字符）。");
  if (normalizedKey.value.length > 512) return tr("主密钥不能超过 256 字节。");
  if (keyTouched.value && (!normalizedKey.value || normalizedKey.value.length % 2)) return tr("每个字节需要两个十六进制字符。");
  return '';
});
const { recoveryInput, recoveryCheck, recoveryNoteId, recoveryError, canSubmitRecovery, onRecoveryBlur, clearRecovery } = useBitlockerRecoveryKey();
const memoryCredential = ref<UnlockCredential | null>(null);
const keyCredential = ref<UnlockCredential | null>(null);
const memoryFields = ref<InstanceType<typeof BitlockerMemoryCandidates> | null>(null);

/** The credential the form holds now; `null` while it is incomplete or malformed. */
const credential = computed<UnlockCredential | null>(() => {
  switch (mode.value) {
    case 'password':
      if (password.value) return { kind: 'password', password: password.value };
      // A VeraCrypt volume may open with keyfiles alone.
      return scheme.value === 'veracrypt' && keyfiles.value.length ? { kind: 'password', password: '' } : null;
    case 'masterKey': return normalizedKey.value && normalizedKey.value.length % 2 === 0 && !keyError.value
      ? { kind: 'masterKey', keyHex: normalizedKey.value } : null;
    case 'recovery': return canSubmitRecovery.value ? { kind: 'recoveryPassword', recoveryPassword: recoveryCheck.value.normalized } : null;
    case 'memory': return memoryCredential.value;
    case 'otherKey': return keyCredential.value;
  }
});

// ── 格式专属信息 ──────────────────────────────────────────────────────────
const encryptedRootPath = ref('');
const inventoryNoteId = useId();
const logicalVolumeIndex = ref(0);
const { inventory, busy: inventoryBusy, error: inventoryError, load: loadInventory, reset: resetInventory } = useFilevaultInventory(
  () => props.open && scheme.value === 'filevault',
  () => ({ imageId: props.info.id, options: {
    ...(selectedPartitionId.value === undefined ? {} : { partitionId: selectedPartitionId.value }),
    ...(encryptedRootPath.value ? { encryptedRootPlistPath: encryptedRootPath.value } : {}),
  } }),
);
const logicalVolumeOptions = computed(() => inventory.value?.volumes.map(volume => ({ value: volume.index, label: `${volume.name || volume.identifier || tr("逻辑卷 {0}", { 0: volume.index })} · ${formatBytes(volume.size)}` })) || []);
const selectedLogicalVolume = computed(() => inventory.value?.volumes.find(volume => volume.index === logicalVolumeIndex.value));
watch(inventory, value => { logicalVolumeIndex.value = value?.volumes[0]?.index ?? 0; });

const apfsLoading = ref(false);
const apfsError = ref('');
const apfsVolumes = ref<ApfsEncryptionVolume[]>([]);
const apfsVolumeIndex = ref(0);
const apfsVolumeOptions = computed(() => apfsVolumes.value.map(volume => ({
  value: volume.index,
  label: volume.name || tr("逻辑卷 {0}", { 0: volume.index }),
})));
watch([() => props.open, scheme, selectedPartitionId, () => props.info.id], async ([open, format, partitionId, imageId], _, onCleanup) => {
  apfsVolumes.value = [];
  apfsError.value = '';
  apfsLoading.value = open && format === 'apfs' && partitionId !== undefined;
  if (!apfsLoading.value || partitionId === undefined) return;
  let stale = false;
  onCleanup(() => { stale = true; });
  try {
    const volumes = await apfsInspect({ imageId, partitionId });
    if (stale) return;
    apfsVolumes.value = volumes.filter(volume => volume.encrypted && !volume.unlocked);
    apfsVolumeIndex.value = apfsVolumes.value[0]?.index ?? 0;
  } catch (error) {
    if (!stale) apfsError.value = String(error);
  } finally {
    if (!stale) apfsLoading.value = false;
  }
});

// ── VeraCrypt ─────────────────────────────────────────────────────────────
const pim = ref(0);
const veracryptHeader = ref<VeracryptHeader>('auto');
const veracryptPrf = ref<VeracryptPrfChoice>('auto');
const keyfiles = ref<string[]>([]);
const pimNoteId = useId();
const headerOptions = computed<UiSelectOption<VeracryptHeader>[]>(() =>
  VERACRYPT_HEADERS.map(value => ({ value, get label() { return veracryptHeaderLabel(value, tr); } })));
const prfOptions = computed<UiSelectOption<VeracryptPrfChoice>[]>(() => VERACRYPT_PRFS
  .filter(value => veracryptPrfFits(veracryptHeader.value, value))
  .map(value => ({ value, get label() { return veracryptPrfLabel(value, tr); } })));
const pimLimit = computed(() => veracryptPimLimit(veracryptHeader.value));
const pimError = computed(() => scheme.value === 'veracrypt' && !(Number.isInteger(pim.value) && pim.value >= 0 && pim.value <= pimLimit.value)
  ? tr("PIM 应为 0–{0} 的整数", { 0: pimLimit.value }) : '');
watch(veracryptHeader, header => { if (!veracryptPrfFits(header, veracryptPrf.value)) veracryptPrf.value = 'auto'; });

let keyfileContext = 0;
async function pickKeyfiles(directory: boolean) {
  const captured = keyfileContext;
  const picked = await openDialog(directory
    ? { directory: true, multiple: true, title: tr("选择密钥文件夹") }
    : { multiple: true, title: tr("选择密钥文件") });
  if (captured !== keyfileContext || !props.open || !picked) return;
  for (const path of Array.isArray(picked) ? picked : [picked]) {
    if (!keyfiles.value.includes(path)) keyfiles.value.push(path);
  }
}

const canUnlock = computed(() => regionValid.value && !!credential.value && !pimError.value
  && (scheme.value !== 'apfs' || apfsVolumes.value.some(volume => volume.index === apfsVolumeIndex.value))
  && (scheme.value !== 'filevault' || (!!selectedLogicalVolume.value && selectedLogicalVolume.value.conversion_status === 'Complete')));

const formId = useId();
const dialogEl = ref<HTMLElement | null>(null);
useDialogFocus(() => props.open, dialogEl);

function clearCredentials() {
  password.value = '';
  masterKey.value = '';
  keyTouched.value = false;
  keyfiles.value = [];
  keyfileContext += 1;
  clearRecovery();
}

// 分区行固定目标；卡片入口优先选第一个检测到且未解锁的加密分区，否则整个镜像。
watch(() => props.open, isOpen => {
  if (!isOpen) {
    clearCredentials();
    return;
  }
  dismissUnlockReminder(props.info.id);
  const start = initialSelection(props.info, props.partitionId);
  if (!start) return;
  scheme.value = start.scheme;
  region.value = start.partitionId === undefined ? 'whole' : String(start.partitionId);
  mode.value = credentialModes(start.scheme)[0]!;
}, { immediate: true });

function unlock() {
  const value = credential.value;
  if (!props.open || !value || !canUnlock.value) return;
  const options: EncryptedUnlockOptions = {
    scheme: scheme.value,
    credential: value,
    ...(selectedPartitionId.value === undefined ? {} : { partitionId: selectedPartitionId.value }),
    ...(scheme.value === 'veracrypt' ? {
      pim: pim.value,
      ...(veracryptHeader.value === 'auto' ? {} : { header: veracryptHeader.value }),
      ...(veracryptPrf.value === 'auto' ? {} : { prf: veracryptPrf.value }),
      ...(keyfiles.value.length ? { keyfiles: [...keyfiles.value] } : {}),
      // A system-encrypted drive opened whole becomes evidence of its own.
      ...(veracryptHeader.value === 'system' && selectedPartitionId.value === undefined ? { target: 'newImage' as const } : {}),
    } : {}),
    ...(scheme.value === 'filevault' ? { ...(inventory.value && inventory.value.volumes.length > 1 ? { target: 'newImage' as const } : {}), logicalVolumeIndex: logicalVolumeIndex.value, ...(encryptedRootPath.value ? { encryptedRootPlistPath: encryptedRootPath.value } : {}) } : {}),
    ...(scheme.value === 'apfs' ? { logicalVolumeIndex: apfsVolumeIndex.value } : {}),
  };
  if (mode.value === 'memory') memoryFields.value?.markSubmitted();
  clearCredentials();
  if (startEncryptedVolumeUnlock(props.info, options)) emit('close');
}

onBeforeUnmount(clearCredentials);

watch([scheme, region, mode], clearCredentials);
</script>

<style scoped>
.unlock-modal { width: min(520px, calc(100vw - 32px)); }
.unlock-form { display: flex; flex: 1; flex-direction: column; min-height: 0; }
.unlock-body { display: grid; gap: 16px; }
fieldset { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; min-width: 0; margin: 0; padding: 0; border: 0; }
fieldset.fixed { grid-template-columns: minmax(0, 1fr); }
label { display: grid; gap: 6px; min-width: 0; color: var(--text-primary); font-size: 12px; font-weight: 500; }
label .ui-select { width: 100%; }
.wide { grid-column: 1 / -1; }
.keyfiles { display: flex; flex-wrap: wrap; gap: 8px; }
.keyfile-list { display: grid; gap: 4px; margin: 0; padding: 0; list-style: none; }
.keyfile-list li { display: flex; align-items: center; gap: 8px; min-width: 0; }
.key-path { flex: 1; min-width: 0; overflow: hidden; color: var(--text-secondary); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.ui-seg-item { flex: 1; }
@media (max-width: 520px) {
  fieldset { grid-template-columns: minmax(0, 1fr); }
}
</style>
