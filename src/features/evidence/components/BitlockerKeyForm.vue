<template>
  <div class="key-form">
    <UiSelect v-model="kind" :options="options" :aria-label="tr('密钥类型')" />
    <template v-if="kind === 'startup'">
      <button class="ui-btn" @click="pickStartupKey">{{ tr("选择 BEK 启动密钥文件") }}</button>
      <p class="ui-field-hint key-path">{{ startupKeyPath || tr("尚未选择文件") }}</p>
    </template>
    <template v-if="kind === 'vmk' || kind === 'fvek'">
      <label :for="inputId">{{ kind === 'vmk' ? tr("VMK（32 字节）") : tr("FVEK（16、32 或 64 字节）") }}</label>
      <input :id="inputId" v-model="keyHex" class="ui-input mono" autocomplete="off" spellcheck="false" :placeholder="tr('十六进制密钥')"
        :aria-invalid="error ? 'true' : undefined" :aria-describedby="noteId" @blur="touched = true" @keydown.enter.prevent="submit" />
      <template v-if="kind === 'fvek'">
        <label :for="`${inputId}-tweak`">{{ tr("TWEAK（仅 Elephant Diffuser 需要）") }}</label>
        <input :id="`${inputId}-tweak`" v-model="tweakHex" class="ui-input mono" autocomplete="off" spellcheck="false" :placeholder="tr('无 Diffuser 时留空')"
          :aria-invalid="tweakError ? 'true' : undefined" :aria-describedby="noteId" @blur="touched = true" />
      </template>
      <p v-if="error || tweakError" :id="noteId" class="ui-field-error" role="alert">{{ error || tweakError }}</p>
      <p v-else :id="noteId" class="ui-field-hint">{{ kind === 'vmk' ? tr("通过 FVEK 认证验证 VMK，不要求恢复密码保护器。") : tr("原始扇区密钥没有认证标签；解锁后请核验文件系统和内容。") }}</p>
    </template>
    <p v-if="kind === 'clear'" class="ui-field-hint">{{ tr("尝试卷内明文密钥保护器，适用于已暂停保护的卷。") }}</p>
    <div class="bde-actions">
      <button class="ui-btn sm" @click="emit('close')">{{ tr("取消") }}</button>
      <button class="ui-btn sm primary" :disabled="!valid" @click="submit">{{ tr("解锁") }}</button>
    </div>
  </div>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { computed, onScopeDispose, ref, useId, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import UiSelect from '../../../components/ui/UiSelect.vue';
import type { BitlockerKeyCredential } from '../model/bitlocker';

const emit = defineEmits<{ submit: [credential: BitlockerKeyCredential]; close: [] }>();
const kind = ref<'startup' | 'vmk' | 'fvek' | 'clear'>('startup');
const options = [
  { value: 'startup' as const, get label() { return tr("BEK 启动密钥"); } }, { value: 'vmk' as const, label: 'VMK' },
  { value: 'fvek' as const, label: 'FVEK / TWEAK' }, { value: 'clear' as const, get label() { return tr("明文密钥"); } },
];
const keyHex = ref('');
const tweakHex = ref('');
const startupKeyPath = ref('');
const touched = ref(false);
const inputId = `bde-key-${useId()}`;
const noteId = `${inputId}-note`;
const normalize = (value: string) => value.replace(/\s/g, '');
const validation = computed(() => {
  const key = normalize(keyHex.value);
  if (!/^[\da-f]+$/i.test(key)) return tr("请输入有效的十六进制密钥。");
  if (kind.value === 'vmk' ? key.length !== 64 : ![32, 64, 128].includes(key.length)) return tr("密钥长度与所选类型不符。");
  return null;
});
const tweakValidation = computed(() => {
  const key = normalize(tweakHex.value);
  return key && (!/^[\da-f]+$/i.test(key) || ![32, 64].includes(key.length)) ? tr("TWEAK 应为 16 或 32 字节的十六进制密钥。") : null;
});
const error = computed(() => touched.value ? validation.value : null);
const tweakError = computed(() => touched.value ? tweakValidation.value : null);
const valid = computed(() => kind.value === 'clear' || (kind.value === 'startup' ? !!startupKeyPath.value : !validation.value && (kind.value !== 'fvek' || !tweakValidation.value)));
let generation = 0;
let disposed = false;
onScopeDispose(() => { disposed = true; generation += 1; });
watch(kind, () => { generation += 1; keyHex.value = ''; tweakHex.value = ''; startupKeyPath.value = ''; touched.value = false; });
async function pickStartupKey() {
  const captured = generation;
  const selected = await open({ multiple: false, title: tr("选择 BEK 启动密钥"), filters: [{ name: tr("BitLocker 启动密钥"), extensions: ['bek', 'BEK'] }] });
  if (!disposed && captured === generation && typeof selected === 'string') startupKeyPath.value = selected;
}
function submit() {
  touched.value = true;
  if (!valid.value) return;
  if (kind.value === 'clear') emit('submit', { kind: 'clear' });
  else if (kind.value === 'startup') emit('submit', { kind: 'startup', startupKeyPath: startupKeyPath.value });
  else if (kind.value === 'vmk') emit('submit', { kind: 'vmk', vmkHex: normalize(keyHex.value) });
  else emit('submit', { kind: 'fvek', fvekHex: normalize(keyHex.value), tweakHex: normalize(tweakHex.value) });
  keyHex.value = ''; tweakHex.value = '';
}
</script>
<style scoped src="../styles/bitlocker-form.css"></style>
<style scoped>
.key-form { display: flex; flex-direction: column; gap: 8px; }
.key-form label { font-size: 12px; color: var(--text-secondary); }
.key-path { overflow-wrap: anywhere; }
</style>
