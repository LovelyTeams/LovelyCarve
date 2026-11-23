<template>
  <div class="key-fields">
    <label>{{ tr("密钥类型") }}
      <UiSelect v-model="kind" :options="options" :disabled="!active" />
    </label>
    <div v-if="kind === 'startup'" class="startup-key">
      <button class="ui-btn ghost sm" type="button" :disabled="!active" @click="pickStartupKey">
        <FileKey :size="13" />{{ startupKeyPath ? tr("更换 BEK 启动密钥文件") : tr("选择 BEK 启动密钥文件") }}
      </button>
      <span v-if="startupKeyPath" class="key-path mono" :title="startupKeyPath">{{ startupKeyPath }}</span>
    </div>
    <template v-if="kind === 'vmk' || kind === 'fvek'">
      <label>{{ kind === 'vmk' ? 'VMK' : 'FVEK' }}
        <input v-model="keyHex" class="ui-input mono" autocomplete="off" spellcheck="false"
          :aria-invalid="error ? 'true' : undefined" :aria-describedby="error ? noteId : undefined" @blur="touched = true" />
      </label>
      <label v-if="kind === 'fvek'">{{ tr("TWEAK（可选）") }}
        <input v-model="tweakHex" class="ui-input mono" autocomplete="off" spellcheck="false"
          :aria-invalid="tweakError ? 'true' : undefined" :aria-describedby="tweakError ? noteId : undefined" @blur="touched = true" />
      </label>
      <p v-if="error || tweakError" :id="noteId" class="ui-field-error" role="alert">{{ error || tweakError }}</p>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, onScopeDispose, ref, useId, watch } from 'vue';
import { open } from '@tauri-apps/plugin-dialog';
import { FileKey } from 'lucide-vue-next';
import { tr } from '../../../composables/useLocale';
import UiSelect from '../../../components/ui/UiSelect.vue';
import type { UnlockCredential } from '../../../lib/ipc/encrypted';

/** BitLocker's other key protectors: a BEK startup key, a VMK, the FVEK with
 *  its TWEAK, or the clear key of a suspended volume. The credential stays
 *  `null` until the input is complete and well-formed. */
const props = defineProps<{ active: boolean }>();
const credential = defineModel<UnlockCredential | null>('credential', { required: true });

type KeyKind = 'startup' | 'vmk' | 'fvek' | 'clear';
const kind = ref<KeyKind>('startup');
const options = [
  { value: 'startup' as const, get label() { return tr("BEK 启动密钥"); } },
  { value: 'vmk' as const, label: 'VMK' },
  { value: 'fvek' as const, label: 'FVEK / TWEAK' },
  { value: 'clear' as const, get label() { return tr("明文密钥"); } },
];
const keyHex = ref('');
const tweakHex = ref('');
const startupKeyPath = ref('');
const touched = ref(false);
const noteId = useId();
const normalize = (value: string) => value.replace(/\s/g, '');
const validation = computed(() => {
  const key = normalize(keyHex.value);
  if (!/^[\da-f]+$/i.test(key)) return tr("请输入有效的十六进制密钥。");
  if (kind.value === 'vmk' && key.length !== 64) return tr("VMK 应为 32 字节。");
  if (kind.value === 'fvek' && ![32, 64, 128].includes(key.length)) return tr("FVEK 应为 16、32 或 64 字节。");
  return null;
});
const tweakValidation = computed(() => {
  const key = normalize(tweakHex.value);
  return key && (!/^[\da-f]+$/i.test(key) || ![32, 64].includes(key.length)) ? tr("TWEAK 应为 16 或 32 字节的十六进制密钥。") : null;
});
const error = computed(() => touched.value ? validation.value : null);
const tweakError = computed(() => touched.value ? tweakValidation.value : null);

const built = computed<UnlockCredential | null>(() => {
  switch (kind.value) {
    case 'clear': return { kind: 'clearKey' };
    case 'startup': return startupKeyPath.value ? { kind: 'startupKey', path: startupKeyPath.value } : null;
    case 'vmk': return validation.value ? null : { kind: 'vmk', keyHex: normalize(keyHex.value) };
    case 'fvek': {
      if (validation.value || tweakValidation.value) return null;
      const tweak = normalize(tweakHex.value);
      return { kind: 'masterKey', keyHex: normalize(keyHex.value), ...(tweak ? { tweakHex: tweak } : {}) };
    }
  }
});
watch(built, value => { credential.value = value; }, { immediate: true });

let generation = 0;
let disposed = false;
onScopeDispose(() => { disposed = true; generation += 1; });
function clear() {
  generation += 1;
  keyHex.value = '';
  tweakHex.value = '';
  startupKeyPath.value = '';
  touched.value = false;
}
watch(kind, clear);
watch(() => props.active, active => { if (!active) clear(); });

async function pickStartupKey() {
  const captured = generation;
  const selected = await open({ multiple: false, title: tr("选择 BEK 启动密钥"), filters: [{ name: tr("BitLocker 启动密钥"), extensions: ['bek', 'BEK'] }] });
  if (!disposed && captured === generation && typeof selected === 'string') startupKeyPath.value = selected;
}
</script>

<style scoped>
.key-fields { display: grid; gap: 12px; min-width: 0; }
.key-fields label { display: grid; gap: 6px; min-width: 0; color: var(--text-primary); font-size: 12px; font-weight: 500; }
.key-fields label .ui-select { width: 100%; }
.startup-key { display: flex; align-items: center; gap: 8px; min-width: 0; }
.key-path { flex: 1; min-width: 0; overflow: hidden; color: var(--text-secondary); font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.ui-field-error { margin: 0; }
</style>
