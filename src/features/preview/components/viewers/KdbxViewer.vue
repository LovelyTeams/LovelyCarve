<template>
  <div class="kdbx-viewer">
    <PreviewState v-if="headerLoading" tone="loading" :title="tr('正在读取 KDBX 头部…')" />
    <PreviewState v-else-if="headerError" tone="error" :icon="CircleAlert" :title="tr('无法读取 KDBX 数据库')" :description="headerError" mono-description />
    <KdbxDatabaseView v-else-if="database" :key="nodeId" :database="database" :credentials="sessionCredentials"
      :image-id="imageId" :node-id="nodeId" @lock="lock" />
    <KdbxUnlockForm v-else-if="header" :key="nodeId" :header="header" :image-id="imageId" :node-id="nodeId"
      :parent-id="parentId ?? null" :unlocking="unlocking" :error="unlockError" @unlock="unlock" />
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../../composables/useLocale';

// KeePass KDBX 的原类型视图：先读取无需密钥的外部头部，用密码、密钥文件或转换密钥解密后
// 浏览群组、条目、历史版本、附件与数据库元数据。
import { ref, watch } from 'vue';
import { CircleAlert } from 'lucide-vue-next';
import { IpcCommandError } from '../../../../lib/ipc/client';
import { fsKdbxHeader, fsKdbxOpen, type KdbxCredentials, type KdbxDatabase, type KdbxHeader } from '../../../../lib/ipc/kdbx';
import type { ImageId, NodeId } from '../../../../lib/ipc/ids';
import PreviewState from '../PreviewState.vue';
import KdbxDatabaseView from './KdbxDatabaseView.vue';
import KdbxUnlockForm, { type KdbxUnlockError } from './KdbxUnlockForm.vue';

const props = defineProps<{
  imageId: ImageId;
  nodeId: NodeId;
  /** 数据库所在目录，列出同目录的候选密钥文件。 */
  parentId?: NodeId | null;
}>();

const header = ref<KdbxHeader | null>(null);
const headerLoading = ref(false);
const headerError = ref<string | null>(null);
const database = ref<KdbxDatabase | null>(null);
// 解密后的读取（附件）改用转换密钥，不再保留密码，也不再重跑密钥派生。
const sessionCredentials = ref<KdbxCredentials>({});
const unlocking = ref(false);
const unlockError = ref<KdbxUnlockError | null>(null);
let sequence = 0;

async function loadHeader() {
  const current = ++sequence;
  header.value = null;
  database.value = null;
  sessionCredentials.value = {};
  headerError.value = null;
  unlockError.value = null;
  unlocking.value = false;
  headerLoading.value = true;
  try {
    const result = await fsKdbxHeader({ imageId: props.imageId, nodeId: props.nodeId });
    if (current === sequence) header.value = result;
  } catch (error) {
    if (current === sequence) headerError.value = String(error);
  } finally {
    if (current === sequence) headerLoading.value = false;
  }
}

async function unlock(credentials: KdbxCredentials) {
  const current = sequence;
  unlocking.value = true;
  unlockError.value = null;
  try {
    const result = await fsKdbxOpen({ imageId: props.imageId, nodeId: props.nodeId, credentials });
    if (current !== sequence) return;
    database.value = result;
    sessionCredentials.value = { transformedKey: result.transformed_key };
  } catch (error) {
    if (current !== sequence) return;
    unlockError.value = {
      message: String(error),
      credentials: error instanceof IpcCommandError && error.code === 'invalidInput',
    };
  } finally {
    if (current === sequence) unlocking.value = false;
  }
}

function lock() {
  database.value = null;
  sessionCredentials.value = {};
  unlockError.value = null;
}

watch(() => [props.imageId, props.nodeId], loadHeader, { immediate: true });
</script>

<style scoped>
.kdbx-viewer {
  display: flex;
  flex-direction: column;
  min-height: 0;
  overflow: hidden;
}
</style>
