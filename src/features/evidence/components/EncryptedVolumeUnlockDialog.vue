<template>
  <Teleport to="body">
    <Transition name="ui-modal">
      <div v-if="open" class="ui-modal-overlay" @click.self="emit('close')">
        <div
          ref="dialogEl"
          class="ui-modal unlock-modal"
          role="dialog"
          aria-modal="true"
          aria-labelledby="encrypted-unlock-title"
          tabindex="-1"
          @keydown.esc.stop="emit('close')"
        >
          <header class="ui-modal-header">
            <span class="ui-modal-icon"><LockKeyhole :size="18" /></span>
            <div class="ui-modal-heading">
              <h2 id="encrypted-unlock-title" class="ui-modal-title">{{ partitionId === null ? tr("解锁加密容器") : tr("解锁分区 {0}", { 0: partitionId }) }}</h2>
            </div>
            <button class="ui-icon-btn" type="button" :title="tr('关闭')" :aria-label="tr('关闭')" @click="emit('close')"><X :size="16" /></button>
          </header>
          <form class="unlock-form" @submit.prevent="unlock">
            <div class="ui-modal-body unlock-body">
              <fieldset :class="{ fixed: partitionId !== null }">
                <label>{{ tr("加密格式") }}
                  <UiSelect v-model="scheme" :options="SCHEME_OPTIONS" />
                </label>
                <label v-if="partitionId === null">{{ tr("加密区域") }}
                  <UiSelect v-model="region" :options="regionOptions" />
                </label>
                <template v-if="scheme === 'filevault'">
                  <label class="wide">{{ tr("外部加密上下文（可选）") }}
                    <input v-model="encryptedRootPath" class="ui-input" :placeholder="tr('EncryptedRoot.plist.wipekey 或 XML 的本地路径')" :aria-invalid="encryptedRootPath && inventoryError ? 'true' : undefined" :aria-describedby="inventoryNoteId" @input="resetInventory" />
                  </label>
                  <div class="wide"><button class="ui-btn ghost sm" type="button" :disabled="inventoryBusy" @click="loadInventory">{{ inventoryBusy ? tr("正在读取卷信息…") : tr("读取卷信息") }}</button></div>
                  <div v-if="inventoryError" :id="inventoryNoteId" class="wide ui-field-error" role="alert">{{ inventoryError }}</div>
                  <p v-if="inventory && inventory.header_offset > 0" class="wide ui-alert">{{ tr("已从备用卷头恢复元数据（区域内偏移") }} {{ inventory.header_offset }}）。</p>
                  <label v-if="inventory" class="wide">{{ tr("逻辑卷") }}
                    <UiSelect v-model="logicalVolumeIndex" :options="logicalVolumeOptions" />
                  </label>
                  <div v-if="selectedLogicalVolume" class="wide ui-field-hint" role="status">
                    {{ tr("状态：") }}{{ selectedLogicalVolume.conversion_status === 'Complete' ? tr("已完成加密") : selectedLogicalVolume.conversion_status || tr("未知") }} {{ tr("· PBKDF2 最低迭代数：") }}{{ selectedLogicalVolume.pbkdf2_iterations.toLocaleString() }} {{ tr("次 · 保护器：") }}{{ selectedLogicalVolume.protectors.map(protectorLabel).join('、') || tr("未发现") }}
                    <p v-if="selectedLogicalVolume.findings.some(finding => finding.code === 'FVDE-WEAK-KDF-ITERATIONS')" class="ui-alert">{{ tr("密码派生迭代数低于 20,000；这是强度观察项，不能据此判断卷已被破解。") }}</p>
                  </div>
                  <p v-if="inventory && inventory.volumes.length > 1" class="wide ui-field-hint">{{ tr("该容器包含多个逻辑卷，所选卷会打开为独立的只读明文会话。") }}</p>
                  <p class="wide ui-field-hint">{{ tr("支持 CoreStorage / FileVault 2。APFS 原生加密和未完成加密的卷暂不支持。") }}</p>
                </template>
                <div v-if="scheme !== 'veracrypt'" class="ui-seg wide" role="group" :aria-label="tr('解锁凭据')">
                  <button class="ui-seg-item" :class="{ active: credential === 'password' }" type="button" :aria-pressed="credential === 'password'" @click="credential = 'password'">{{ tr("密码") }}</button>
                  <button class="ui-seg-item" :class="{ active: credential === 'masterKey' }" type="button" :aria-pressed="credential === 'masterKey'" @click="credential = 'masterKey'">{{ tr("主密钥") }}</button>
                </div>
                <label v-if="!usesMasterKey" class="wide">{{ scheme === 'filevault' ? tr("密码 / 个人恢复密码") : tr("密码") }}<input v-model="password" class="ui-input" type="password" autocomplete="off" required maxlength="4096" /></label>
                <label v-else class="wide">{{ tr("主密钥（十六进制）") }}
                  <input v-model="masterKey" class="ui-input mono" type="password" autocomplete="off" required maxlength="1024" :aria-invalid="keyError ? 'true' : undefined" :aria-describedby="keyNoteId" @blur="keyTouched = true" />
                  <span v-if="keyError" :id="keyNoteId" class="ui-field-error" role="alert">{{ keyError }}</span>
                  <span v-else :id="keyNoteId" class="ui-field-hint">{{ scheme === 'filevault' ? tr("16 字节 VMK 或 32 字节 XTS 密钥对；会验证解密后的卷头。") : tr("填写已恢复的卷主密钥，可粘贴带空格的十六进制字节。") }}</span>
                </label>
                <template v-if="scheme === 'veracrypt'">
                  <label>{{ tr("PIM（0 为默认）") }}<input v-model.number="pim" class="ui-input mono" type="number" min="0" max="1000" step="1" required /></label>
                  <label class="ui-check check"><input v-model="hidden" type="checkbox" />{{ tr("打开隐藏卷") }}</label>
                </template>
              </fieldset>
            </div>
            <footer class="ui-modal-footer">
              <button class="ui-btn" type="button" @click="emit('close')">{{ tr("取消") }}</button>
              <button class="ui-btn primary" type="submit" :disabled="!canUnlock">{{ tr("开始后台解锁") }}</button>
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
import { LockKeyhole, X } from 'lucide-vue-next';
import type { ImageInfo } from '../../../lib/ipc/image';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import { startEncryptedVolumeUnlock } from '../composables/useEncryptedVolumeUnlocks';
import { useFilevaultInventory } from '../composables/useFilevaultInventory';
import { formatBytes } from '../../../utils/format';
import UiSelect, { type UiSelectOption } from '../../../components/ui/UiSelect.vue';

const props = defineProps<{
  open: boolean;
  info: ImageInfo;
  /** 从分区行打开时预选的分区；从卷卡标题打开为 null（整个镜像）。 */
  partitionId: number | null;
}>();
const emit = defineEmits<{ close: [] }>();
const SCHEME_OPTIONS: UiSelectOption<'luks' | 'veracrypt' | 'filevault'>[] = [
  { value: 'luks', label: 'LUKS1 / LUKS2' },
  { value: 'veracrypt', label: 'VeraCrypt' },
  { value: 'filevault', label: 'FileVault 2 / CoreStorage' },
];
const scheme = ref<'luks' | 'veracrypt' | 'filevault'>('luks');
const region = ref('whole');
const regionOptions = computed<UiSelectOption<string>[]>(() => [
  { value: 'whole', get label() { return tr("整个镜像 / 容器"); } },
  ...props.info.partitions.map(partition => ({ value: String(partition.id), get label() { return tr("分区 {0} · {1}", { 0: partition.id, 1: partition.label || partition.kind }); } })),
]);
const password = ref('');
const credential = ref<'password' | 'masterKey'>('password');
const masterKey = ref('');
const keyTouched = ref(false);
const keyNoteId = useId();
const usesMasterKey = computed(() => scheme.value !== 'veracrypt' && credential.value === 'masterKey');
const normalizedKey = computed(() => masterKey.value.replace(/\s/g, ''));
const keyError = computed(() => {
  if (/[^0-9a-f]/i.test(normalizedKey.value)) return tr("主密钥只能包含十六进制字符 0–9、A–F。");
  if (scheme.value === 'filevault' && (keyTouched.value || normalizedKey.value.length > 0) && ![32, 64].includes(normalizedKey.value.length)) return tr("FileVault 密钥需要 16 或 32 字节（32 或 64 个十六进制字符）。");
  if (normalizedKey.value.length > 512) return tr("主密钥不能超过 256 字节。");
  if (keyTouched.value && (!normalizedKey.value || normalizedKey.value.length % 2)) return tr("每个字节需要两个十六进制字符。");
  return '';
});
const encryptedRootPath = ref('');
const inventoryNoteId = useId();
const logicalVolumeIndex = ref(0);
const { inventory, busy: inventoryBusy, error: inventoryError, load: loadInventory, reset: resetInventory } = useFilevaultInventory(
  () => props.open && scheme.value === 'filevault',
  () => ({ imageId: props.info.id, options: {
    ...(props.partitionId !== null ? { partitionId: props.partitionId } : region.value === 'whole' ? {} : { partitionId: Number(region.value) }),
    ...(encryptedRootPath.value ? { encryptedRootPlistPath: encryptedRootPath.value } : {}),
  } }),
);
const logicalVolumeOptions = computed(() => inventory.value?.volumes.map(volume => ({ value: volume.index, label: `${volume.name || volume.identifier || tr("逻辑卷 {0}", { 0: volume.index })} · ${formatBytes(volume.size)}` })) || []);
const selectedLogicalVolume = computed(() => inventory.value?.volumes.find(volume => volume.index === logicalVolumeIndex.value));
watch(inventory, value => { logicalVolumeIndex.value = value?.volumes[0]?.index ?? 0; });
function protectorLabel(kind: string) { return ({ password: tr("密码"), recovery: tr("个人恢复"), institutional: tr("机构恢复"), unknown: tr("未知") } as Record<string, string>)[kind] || kind; }
const canUnlock = computed(() => (scheme.value !== 'filevault' || (!!selectedLogicalVolume.value && selectedLogicalVolume.value.conversion_status === 'Complete')) && (usesMasterKey.value
  ? !!normalizedKey.value && normalizedKey.value.length % 2 === 0 && !keyError.value
  : !!password.value));
const pim = ref(0);
const hidden = ref(false);
const dialogEl = ref<HTMLElement | null>(null);

useDialogFocus(() => props.open, dialogEl);

// 每次打开都从触发它的位置开始：分区行预选该分区（LUKS 分区同时选 LUKS），
// 卷卡标题选整个镜像。关闭即清空密码；后台任务不持有弹窗。
watch(() => props.open, isOpen => {
  if (!isOpen) {
    password.value = '';
    masterKey.value = '';
    keyTouched.value = false;
    return;
  }
  const partition = props.info.partitions.find(candidate => candidate.id === props.partitionId);
  if (partition?.kind === 'luks' || partition?.kind === 'filevault') scheme.value = partition.kind;
  region.value = partition ? String(partition.id) : 'whole';
}, { immediate: true });

function unlock() {
  if (!canUnlock.value) return;
  const options = {
    scheme: scheme.value,
    ...(usesMasterKey.value ? { masterKeyHex: normalizedKey.value } : { password: password.value }),
    ...(props.partitionId !== null ? { partitionId: props.partitionId } : region.value === 'whole' ? {} : { partitionId: Number(region.value) }),
    ...(scheme.value === 'veracrypt' ? { pim: pim.value, hidden: hidden.value } : {}),
    ...(scheme.value === 'filevault' ? { ...(inventory.value && inventory.value.volumes.length > 1 ? { target: 'newImage' as const } : {}), logicalVolumeIndex: logicalVolumeIndex.value, ...(encryptedRootPath.value ? { encryptedRootPlistPath: encryptedRootPath.value } : {}) } : {}),
  };
  password.value = '';
  masterKey.value = '';
  keyTouched.value = false;
  if (startEncryptedVolumeUnlock(props.info, options)) emit('close');
}

onBeforeUnmount(() => {
  password.value = '';
  masterKey.value = '';
  // The backend job remains visible in the task center when changing views.
});

watch([scheme, credential], () => {
  password.value = '';
  masterKey.value = '';
  keyTouched.value = false;
});
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
.check { display: flex; align-self: end; height: 30px; }
@media (max-width: 520px) {
  fieldset { grid-template-columns: minmax(0, 1fr); }
}
</style>
