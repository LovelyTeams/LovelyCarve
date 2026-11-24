<template>
  <div class="kdbx-unlock">
    <div class="report-viewer unlock-column">
      <header class="report-head">
        <div class="report-icon"><LockKeyhole :size="20" /></div>
        <div class="report-title">
          <h3 class="report-name">{{ tr("KeePass 数据库") }}</h3>
          <div class="report-badges">
            <span class="ui-badge sm mono">KDBX {{ header.version }}</span>
            <span class="ui-badge sm mono">{{ header.cipher }}</span>
            <span class="ui-badge sm mono">{{ header.kdf.name }}</span>
            <span v-if="header.sha256_valid === false" class="ui-badge sm warn"><AlertTriangle :size="11" />{{ tr("头部 SHA-256 不符") }}</span>
          </div>
        </div>
      </header>

      <form class="unlock-card" @submit.prevent="submit">
        <div class="ui-field">
          <label class="ui-field-label" :for="passwordId">{{ tr("主密码") }}</label>
          <div class="input-row">
            <input :id="passwordId" ref="passwordInput" v-model="password" :type="showPassword ? 'text' : 'password'"
              class="ui-input mono" autocomplete="off" spellcheck="false"
              :aria-invalid="credentialError ? 'true' : undefined" :aria-describedby="credentialError ? errorId : undefined" />
            <button type="button" class="ui-icon-btn sm" :aria-pressed="showPassword"
              :title="showPassword ? tr('隐藏密码') : tr('显示密码')" :aria-label="showPassword ? tr('隐藏密码') : tr('显示密码')"
              @click="showPassword = !showPassword">
              <EyeOff v-if="showPassword" :size="14" /><Eye v-else :size="14" />
            </button>
          </div>
        </div>

        <div class="ui-field">
          <span :id="keyfileLabelId" class="ui-field-label">{{ tr("密钥文件") }}</span>
          <div class="input-row">
            <UiSelect v-model="keyfileChoice" class="sm keyfile-select" :options="keyfileOptions" :aria-labelledby="keyfileLabelId" />
            <button type="button" class="ui-btn sm" @click="pickHostKeyfile"><FileKey :size="13" />{{ tr("选择主机文件") }}</button>
          </div>
          <p v-if="siblingError" class="ui-field-error"><CircleAlert :size="12" /><span>{{ siblingError }}</span></p>
        </div>

        <details v-disclosure-motion class="ui-disclosure transformed" :open="transformedKey !== ''">
          <summary><ChevronRight :size="13" class="ui-chevron" />{{ tr("转换密钥") }}</summary>
          <input v-model="transformedKey" class="ui-input mono transformed-input" autocomplete="off" spellcheck="false"
            :aria-label="tr('转换密钥')" :placeholder="'0'.repeat(64)"
            :aria-invalid="transformedInvalid ? 'true' : undefined" :aria-describedby="transformedInvalid ? transformedErrorId : undefined" />
          <p v-if="transformedInvalid" :id="transformedErrorId" class="ui-field-error" role="alert">
            <CircleAlert :size="12" /><span>{{ tr("转换密钥应为 64 个十六进制字符") }}</span>
          </p>
        </details>

        <p v-if="credentialError" :id="errorId" class="ui-field-error" role="alert"><CircleAlert :size="12" /><span>{{ error?.message }}</span></p>
        <p v-else-if="error" class="ui-alert danger" role="alert"><CircleAlert :size="14" /><span>{{ error.message }}</span></p>

        <button type="submit" class="ui-btn primary submit" :disabled="unlocking || transformedInvalid">
          <Loader2 v-if="unlocking" :size="14" class="spin" /><KeyRound v-else :size="14" />
          {{ unlocking ? tr("解密中…") : tr("解密") }}
        </button>
      </form>

      <section class="section">
        <div class="section-head"><h4 class="section-title">{{ tr("外部头部") }}</h4></div>
        <KdbxDetailList :rows="details" />
      </section>

      <section v-if="header.warnings.length" class="ui-alert warn report-warnings" role="note">
        <AlertTriangle :size="14" />
        <div class="warn-body">
          <p v-for="(warning, index) in header.warnings" :key="index" class="warn-line mono">{{ warning }}</p>
        </div>
      </section>
    </div>
  </div>
</template>

<script lang="ts">
export interface KdbxUnlockError {
  message: string;
  /** The credentials were rejected (shown on the password field). */
  credentials: boolean;
}
</script>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

import { computed, nextTick, onMounted, ref, useId, watch } from 'vue';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { AlertTriangle, ChevronRight, CircleAlert, Eye, EyeOff, FileKey, KeyRound, Loader2, LockKeyhole } from 'lucide-vue-next';
import { vDisclosureMotion } from '../../../../composables/uiMotionDirectives';
import { fsChildrenPage } from '../../../../lib/ipc/fs/query';
import type { FsNode } from '../../../../lib/ipc/fs/types';
import type { KdbxCredentials, KdbxHeader } from '../../../../lib/ipc/kdbx';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';
import UiSelect, { type UiSelectOption } from '../../../../components/ui/UiSelect.vue';
import { headerDetails } from '../../model/kdbx';
import KdbxDetailList from './KdbxDetailList.vue';

const props = defineProps<{
  header: KdbxHeader;
  imageId: ImageId;
  nodeId: NodeId;
  parentId: NodeId | null;
  unlocking: boolean;
  error: KdbxUnlockError | null;
}>();
const emit = defineEmits<{ unlock: [credentials: KdbxCredentials] }>();

/** Keyfiles KeePass reads whole; larger files are not offered from the evidence. */
const KEYFILE_CAP = 64 * 1024 * 1024;
const SIBLING_PAGE = 500;
const KEYFILE_EXTENSIONS = ['key', 'keyx', 'keyfile', 'xml'];

const passwordId = useId();
const errorId = useId();
const keyfileLabelId = useId();
const transformedErrorId = useId();
const passwordInput = ref<HTMLInputElement | null>(null);
const password = ref('');
const showPassword = ref(false);
const transformedKey = ref('');
/** `''`, `node:<id>` or `host:<path>`. */
const keyfileChoice = ref('');
const hostKeyfile = ref<string | null>(null);
const siblings = ref<FsNode[]>([]);
const siblingError = ref<string | null>(null);

const details = computed(() => headerDetails(props.header, tr));
const credentialError = computed(() => !!props.error?.credentials);
const normalizedTransformed = computed(() => transformedKey.value.replace(/\s+/g, ''));
const transformedInvalid = computed(() => normalizedTransformed.value !== '' && !/^[0-9a-fA-F]{64}$/.test(normalizedTransformed.value));

function extensionOf(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/** Files that look like keyfiles first, then the rest by name. */
const keyfileOptions = computed<UiSelectOption<string>[]>(() => {
  const ranked = [...siblings.value].sort((left, right) => {
    const rank = (node: FsNode) => (KEYFILE_EXTENSIONS.includes(extensionOf(node.name)) ? 0 : 1);
    return rank(left) - rank(right) || left.name.localeCompare(right.name);
  });
  return [
    { value: '', label: tr('不使用密钥文件') },
    ...(hostKeyfile.value ? [{ value: `host:${hostKeyfile.value}`, label: hostKeyfile.value }] : []),
    ...ranked.map(node => ({ value: `node:${node.id}`, label: node.name })),
  ];
});

async function loadSiblings() {
  siblings.value = [];
  siblingError.value = null;
  if (!props.parentId) return;
  try {
    const page = await fsChildrenPage({ imageId: props.imageId, parentId: props.parentId, pageSize: SIBLING_PAGE });
    siblings.value = page.items.filter(node => node.kind === 'file' && node.id !== props.nodeId && node.size > 0 && node.size <= KEYFILE_CAP);
  } catch (error) {
    siblingError.value = String(error);
  }
}

let pickContext = 0;
async function pickHostKeyfile() {
  const captured = ++pickContext;
  const picked = await openDialog({ multiple: false, directory: false, title: tr('选择密钥文件') });
  if (captured !== pickContext || typeof picked !== 'string') return;
  hostKeyfile.value = picked;
  keyfileChoice.value = `host:${picked}`;
}

function submit() {
  if (props.unlocking || transformedInvalid.value) return;
  const credentials: KdbxCredentials = {};
  if (normalizedTransformed.value) {
    credentials.transformedKey = normalizedTransformed.value;
  } else {
    if (password.value) credentials.password = password.value;
    const choice = keyfileChoice.value;
    if (choice.startsWith('node:')) credentials.keyfile = { kind: 'evidence', nodeId: choice.slice(5) as NodeId };
    else if (choice.startsWith('host:')) credentials.keyfile = { kind: 'host', path: choice.slice(5) };
  }
  emit('unlock', credentials);
}

// 凭据错误时把焦点交给密码框重输；初次显示不抢焦点，文件列表的键盘浏览不被打断。
watch(() => props.error, error => {
  if (error?.credentials) void nextTick(() => passwordInput.value?.select());
});

onMounted(() => void loadSiblings());
</script>

<style scoped src="../../../../styles/report-viewer.css"></style>
<style scoped>
.kdbx-unlock {
  display: flex;
  flex: 1;
  min-height: 0;
  overflow: auto;
}

.unlock-column {
  width: 100%;
  max-width: 640px;
  margin: 0 auto;
  overflow: visible;
}

.unlock-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px 18px;
  background: var(--surface-secondary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-lg);
  box-shadow: var(--card-shadow);
}

.input-row {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.input-row > .ui-input,
.keyfile-select {
  flex: 1;
  min-width: 0;
}

.transformed {
  font-size: 12px;
}

.transformed summary {
  color: var(--text-secondary);
  padding: 2px 0;
}

.transformed summary:hover {
  color: var(--text-primary);
}

.transformed-input {
  width: 100%;
  margin-top: 8px;
}

.submit {
  align-self: stretch;
}
</style>
