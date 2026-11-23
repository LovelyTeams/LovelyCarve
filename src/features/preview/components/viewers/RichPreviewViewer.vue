<template>
  <div class="rich-preview">
    <PreviewState v-if="loading" tone="loading" :title="phaseLabel" />
    <PreviewState v-else-if="tooLarge" :icon="FileWarning" :title="tr('文件过大，不渲染预览')"
      :description="formatBytes(node.size)" />
    <PreviewState v-else-if="empty" :icon="FileX" :title="tr('空文件')" />
    <PreviewState v-else-if="error" tone="error" :icon="CircleAlert" :title="tr('无法渲染此文件')" :description="error" mono-description />
    <!-- Flyfish File Viewer 自带工具栏 / 渲染面与 Shadow DOM 隔离，整个容器交由其管理；
         theme 经 options 注入，切换主题时以 :key 重挂（库对 options 的响应性未验证）。 -->
    <FileViewer
      v-else
      :key="`${node.id}:${currentTheme}:${locale}`"
      class="rich-viewer"
      :file="sourceFile ?? undefined"
      :url="sourceUrl ?? undefined"
      :name="node.name"
      :options="viewerOptions"
    />
  </div>
</template>

<script setup lang="ts">
import { tr, useLocale } from '../../../../composables/useLocale';

// 「rich」原类型视图：文档 / 办公 / 邮件 / 非 zip 压缩包 / CAD / 3D / 图表等格式交给
// Flyfish File Viewer（@file-viewer/vue3-full）在浏览器内渲染。渲染器按扩展名懒加载，
// 扩展名路由以 `name` prop 为准（它覆盖从 url / File 推导的文件名——大文件走临时文件
// 路径时，fs_media_temp 会把 `a.tar.gz` 压扁成 `<md5>.bin`，靠 `name` 修正路由）。
//
// 小文件走可取消的 16 MiB 内存读取；PDF 可用 asset Range 流式读取。
// 非流式渲染器限制为 64 MiB，避免 URL 路径被库整读为数 GiB。
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import { CircleAlert, FileWarning, FileX } from 'lucide-vue-next';
import PreviewState from '../PreviewState.vue';
import { FileViewer } from '@file-viewer/vue3-full';
import type { FileViewerOptions } from '@file-viewer/vue3-full';
import '@file-viewer/vue3/dist/file-viewer3.css';
import type { FsNode } from '../../../../lib/ipc/fs/types';
import { createPreviewRequest } from '../../composables/previewRequest';
import { mediaStreamUrl } from '../../../../lib/ipc/media';
import { formatBytes } from '../../../../utils/format';
import { useTheme } from '../../../../composables/useTheme';
import type { ImageId } from '../../../../lib/ipc/ids';

const props = defineProps<{
  imageId: ImageId;
  node: FsNode;
}>();

// 原始字节预算；后端独立验证内存读取和磁盘缓存上限。
const READ_CAP = 16 * 1024 * 1024;
const NON_STREAM_CAP = 64 * 1024 * 1024;
const TEMP_CAP = 4 * 1024 * 1024 * 1024;

const { currentTheme } = useTheme();
const { locale } = useLocale();

const loading = ref(true);
const error = ref<string | null>(null);
const tooLarge = ref(false);
const empty = ref(false);
const sourceFile = ref<File | null>(null);
const sourceUrl = ref<string | null>(null);
// 加载阶段（驱动进度条文案）：读取字节 → 物化临时文件。渲染由库内部负责（懒加载 + 解析）。
const phase = ref<'read' | 'materialize'>('read');
const phaseLabel = computed(() =>
  phase.value === 'read' ? tr("正在读取文件字节…") : tr("正在准备预览文件…"),
);

// 卸载守卫 + 单调请求序号：父级以 :key=node.id 重挂本组件（每个文件一个新实例），故 nodeId
// 通常不在实例内变化；这两个守卫兜底处理「卸载后异步落地」与「未重挂时切文件」两种竞态。
let disposed = false;
let loadSeq = 0;
let request: ReturnType<typeof createPreviewRequest> | null = null;
function cancelInFlight() { request?.cancel(); request = null; }

async function load() {
  const my = ++loadSeq;
  error.value = null;
  tooLarge.value = false;
  empty.value = false;
  sourceFile.value = null;
  sourceUrl.value = null;
  cancelInFlight();

  if (props.node.size === 0) {
    empty.value = true;
    loading.value = false;
    return;
  }
  const streaming = props.node.name.toLowerCase().endsWith('.pdf');
  if (props.node.size > (streaming ? TEMP_CAP : NON_STREAM_CAP)) {
    tooLarge.value = true;
    loading.value = false;
    return;
  }

  loading.value = true;
  const nodeId = props.node.id;
  const activeRequest = createPreviewRequest();
  request = activeRequest;
  try {
    if (props.node.size <= READ_CAP) {
      phase.value = 'read';
      const buf = await activeRequest.read({ imageId: props.imageId, nodeId });
      if (!buf || disposed || my !== loadSeq) return;
      // 扩展名路由依赖文件名，故必须包成带名的 File（裸 ArrayBuffer 拿不到扩展名）。
      sourceFile.value = new File([buf], props.node.name);
    } else {
      phase.value = 'materialize';
      const path = await activeRequest.materialize({ imageId: props.imageId, nodeId });
      if (!path || disposed || my !== loadSeq) return;
      sourceUrl.value = mediaStreamUrl(path);
    }
    if (disposed || my !== loadSeq) return;
    loading.value = false;
  } catch (e) {
    if (disposed || my !== loadSeq) return;
    error.value = String(e);
    loading.value = false;
  }
}

watch(() => [props.imageId, props.node.id], () => load(), { immediate: true });

onBeforeUnmount(() => {
  disposed = true;
  cancelInFlight();
});

const viewerOptions = computed<FileViewerOptions>(() => ({
  i18n: { locale: locale.value === 'en' ? 'en-US' : 'zh-CN' },
  theme: currentTheme.value,
  locale: locale.value === 'en' ? 'en-US' : 'zh-CN',
  // 默认（auto）即 Shadow DOM：库的样式不渗入 app，app 的 reset 也伤不到库内 UI。
  styleIsolation: 'auto',
  pdf: {
    // 大文件走 asset 协议 URL 时启用 Range 流式，避免整文件 fetch 进内存。
    streaming: true,
  },
  archive: {
    maxEntryPreviewSize: 64 * 1024 * 1024,
  },
  ui: { density: 'compact' },
  toolbar: {
    // 主题由宿主（useTheme）统一管理，隐藏库内置的明暗切换按钮。
    theme: false,
  },
}));
</script>

<style scoped>
.rich-preview {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--bg-primary);
}

.rich-viewer {
  flex: 1;
  min-height: 0;
}
</style>
