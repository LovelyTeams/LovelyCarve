<template>
  <div class="media-wrap">
    <!-- 视频 / 音频：经 asset 协议流式预览（不整文件入内存、可拖动进度）。
         后端 fs_media_temp 先把节点物化到临时文件，再由 <video>/<audio>
         通过 asset 协议按需取字节范围（支持跳转 / 大文件）。 -->
    <template v-if="isStreamMedia">
      <div v-if="streamLoading" class="state-row mono">{{ tr("准备预览…") }}</div>
      <div v-else-if="streamError" class="state-row mono error">{{ streamError }}</div>
      <video
        v-else-if="streamUrl && category === 'video'"
        :key="node.id"
        class="media-view"
        :src="streamUrl"
        controls
        playsinline
        @error="emit('mediaError')"
      />
      <audio
        v-else-if="streamUrl && category === 'audio'"
        :key="node.id"
        class="media-audio"
        :src="streamUrl"
        controls
        @error="emit('mediaError')"
      />
    </template>
    <!-- 图片：整文件读入内存以 Blob 预览（受 MEDIA_CAP 约束）。 -->
    <template v-else>
      <div v-if="mediaTooLarge" class="state-row mono">
        {{ tr("文件过大（") }}{{ formatBytes(node.size) }}{{ tr("），无法预览，请导出后查看。") }}
      </div>
      <div v-else-if="mediaLoading" class="state-row mono">{{ tr("加载中…") }}</div>
      <div v-else-if="mediaError" class="state-row mono error">{{ mediaError }}</div>
      <template v-else-if="mediaUrl">
      <div v-if="category === 'image'" class="qr-image-area">
        <div class="qr-stage">
          <img
            :ref="elements.image"
            class="media-view"
            :src="mediaUrl"
            :alt="node.name"
            @load="emit('imageLoad')"
          />
          <svg
            v-if="recognition.qrResult && recognition.qrResult.codes.length && recognition.imageNatural"
            class="qr-overlay"
            :viewBox="`0 0 ${recognition.imageNatural.w} ${recognition.imageNatural.h}`"
            preserveAspectRatio="xMidYMid meet"
          >
            <polygon
              v-for="(c, i) in recognition.qrResult.codes"
              :key="i"
              v-show="c.points.length >= 3"
              class="qr-box"
              :class="{ active: qrActiveIndex === i }"
              :points="svgPoints(c.points)"
            />
          </svg>
          <svg
            v-if="recognition.ocrResult && recognition.ocrResult.regions.length && recognition.imageNatural"
            class="ocr-overlay"
            :viewBox="`0 0 ${recognition.imageNatural.w} ${recognition.imageNatural.h}`"
            preserveAspectRatio="xMidYMid meet"
          >
            <polygon
              v-for="(r, i) in recognition.ocrResult.regions"
              :key="i"
              v-show="r.points.length >= 3"
              class="ocr-box"
              :points="svgPoints(r.points)"
            />
          </svg>
          <!-- 可选中文字层：把识别出的文本以透明字叠在原图对应位置，鼠标左键划过即可
               像「实况文本」一样直接选中 / 复制（::selection 高亮透出底图文字）。 -->
          <div
            v-if="recognition.ocrResult && recognition.ocrResult.regions.length && recognition.imageNatural"
            :ref="elements.textLayer"
            class="ocr-text-layer"
          >
            <span
              v-for="(it, i) in recognition.ocrTextItems"
              :key="i"
              class="ocr-text-span"
              :style="it.style"
            >{{ it.text }}</span>
          </div>
        </div>

        <div v-if="recognition.qrScanning || recognition.qrResult || recognition.qrError" class="qr-results">
          <div class="qr-results-head">
            <QrCode :size="13" />
            <span class="qr-results-title">{{ tr("二维码识别") }}</span>
            <span v-if="recognition.qrResult" class="ui-badge sm mono qr-pill">
              {{ recognition.qrResult.codes.length }} {{ tr("个 ·") }}
              {{ recognition.qrResult.detector === 'wechat' ? 'WeChat' : tr("标准") }} ·
              {{ recognition.qrResult.attempts }} {{ tr("次") }}
              <template v-if="recognition.qrResult.cancelled"> {{ tr("· 已取消") }}</template>
            </span>
            <span class="qr-spacer"></span>
            <label class="qr-expected" :title="tr('期望识别的二维码数量（达到即停止扫描）')">
              {{ tr("数量") }}
              <input
                class="ui-input sm mono qr-expected-input"
                type="number"
                min="1"
                max="20"
                v-model.number="qrExpected"
                :disabled="recognition.qrScanning"
              />
            </label>
            <button v-if="recognition.qrScanning" class="ui-btn sm danger-soft" :title="tr('取消扫描')" @click="emit('cancelQr')">
              <X :size="12" />{{ tr("取消") }}
            </button>
          </div>

          <div v-if="recognition.qrScanning" class="qr-progress mono">
            {{ tr("扫描中") }}
            <template v-if="recognition.qrProgress">
              {{ tr("{0}/{1}（已找到 {2}）", { 0: recognition.qrProgress.attempts, 1: recognition.qrProgress.total, 2: recognition.qrProgress.found }) }}
            </template>
          </div>
          <div v-else-if="recognition.qrError" class="qr-empty mono">{{ recognition.qrError }}</div>
          <ul v-else-if="recognition.qrResult && recognition.qrResult.codes.length" class="qr-list">
            <li
              v-for="(c, i) in recognition.qrResult.codes"
              :key="i"
              class="qr-item"
              :class="{ active: qrActiveIndex === i }"
              @mouseenter="qrActiveIndex = i"
              @mouseleave="qrActiveIndex = null"
            >
              <div class="qr-item-main">
                <span class="qr-idx mono">#{{ i + 1 }}</span>
                <span class="qr-text mono">{{ c.text }}</span>
              </div>
              <div class="qr-item-meta">
                <span class="qr-transform mono">{{ c.transform }}</span>
                <button class="ui-icon-btn sm bordered" :title="tr('复制内容')" :aria-label="tr('复制内容')" @click="emit('copyQr', c.text, i)">
                  <Check v-if="recognition.qrCopiedIndex === i" :size="12" />
                  <Copy v-else :size="12" />
                </button>
              </div>
            </li>
          </ul>
        </div>

        <div v-if="recognition.ocrScanning || recognition.ocrResult || recognition.ocrError" class="ocr-results">
          <div class="ocr-results-head">
            <ScanText :size="13" />
            <span class="ocr-results-title">{{ tr("文字识别") }}</span>
            <span class="ocr-spacer"></span>
            <button
              v-if="recognition.ocrFullText"
              class="ui-btn sm"
              :title="recognition.ocrCopiedAll ? tr('已复制') : tr('复制文本')"
              @click="emit('copyOcr')"
            >
              <Check v-if="recognition.ocrCopiedAll" :size="12" />
              <Copy v-else :size="12" />
              {{ tr("复制") }}
            </button>
          </div>

          <div v-if="recognition.ocrScanning" class="ocr-progress mono">{{ tr("识别中…") }}</div>
          <div v-else-if="recognition.ocrError" class="ocr-empty mono">{{ recognition.ocrError }}</div>
          <pre v-else-if="recognition.ocrFullText" class="ocr-text-block">{{ recognition.ocrFullText }}</pre>
        </div>
      </div>
      </template>
    </template>
  </div>


</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import type { Ref } from 'vue';
import { X, Copy, Check, QrCode, ScanText } from 'lucide-vue-next';
import type { FsNode } from '../../../lib/ipc/fs/types';
import type { PreviewCategory } from '../model/preview';
import { formatPreviewBytes as formatBytes } from '../model/preview';
import { svgPoints, type ImageRecognitionView } from '../model/imageRecognition';

defineProps<{
  node: FsNode; category: PreviewCategory; isStreamMedia: boolean; mediaTooLarge: boolean;
  mediaUrl: string | null; mediaLoading: boolean; mediaError: string | null;
  streamUrl: string | null; streamLoading: boolean; streamError: string | null;
  recognition: ImageRecognitionView;
  elements: { image: Ref<HTMLImageElement | null>; textLayer: Ref<HTMLElement | null> };
}>();
const qrExpected = defineModel<number>('qrExpected', { required: true });
const qrActiveIndex = defineModel<number | null>('qrActiveIndex', { required: true });
const emit = defineEmits<{ mediaError: []; imageLoad: []; cancelQr: []; copyQr: [text: string, index: number]; copyOcr: [] }>();
</script>

<style scoped>
.mono { font-family: var(--font-mono); font-size: 12px; }
.state-row { padding: 24px; text-align: center; color: var(--text-secondary); }
.state-row.error { color: var(--sev-critical-fg); }
.media-wrap {
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: auto;
  padding: 12px;
}

.media-view {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
  border-radius: var(--radius-sm);
}

.media-audio {
  width: 80%;
  max-width: 480px;
}

/* ---- 二维码识别：图片舞台 + 检测框叠层 + 结果面板 ---- */
.qr-image-area {
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-width: 100%;
  max-height: 100%;
  min-height: 0;
}

/* 舞台收缩包裹图片本体，叠层 SVG 以图片自然尺寸为 viewBox 精确对齐。 */
.qr-stage {
  position: relative;
  display: inline-flex;
  align-self: center;
  max-width: 100%;
  min-height: 0;
}

.qr-overlay {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  pointer-events: none;
}

.qr-box {
  fill: transparent;
  stroke: var(--primary-color);
  /* viewBox 用原图像素，描边不随缩放变粗。 */
  stroke-width: 3;
  vector-effect: non-scaling-stroke;
  transition: stroke var(--transition-fast), fill var(--transition-fast);
}

.qr-box.active {
  stroke: var(--sev-critical-fg);
  fill: var(--sev-critical-bg);
}

.qr-results {
  flex-shrink: 0;
  border: 1px solid var(--border-color);
  background: var(--surface-secondary);
  border-radius: var(--radius-md);
  padding: 10px 12px;
}

.qr-results-head {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-primary);
}

.qr-results-title {
  font-size: 12px;
  font-weight: 600;
}

.qr-spacer {
  flex: 1;
}

.qr-expected {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  color: var(--text-secondary);
}

.qr-expected-input {
  width: 52px;
  height: 24px;
  padding: 0 6px;
}

.qr-progress,
.qr-empty {
  margin-top: 8px;
  font-size: 11px;
  color: var(--text-secondary);
}

.qr-list {
  list-style: none;
  margin: 8px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 200px;
  overflow: auto;
}

.qr-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 6px 8px;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-primary);
  transition: border-color var(--transition-fast), background var(--transition-fast);
}

.qr-item.active {
  border-color: var(--primary-color);
  background: var(--sidebar-active-bg);
}

.qr-item-main {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.qr-idx {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--text-secondary);
}

.qr-text {
  font-size: 12px;
  color: var(--text-primary);
  word-break: break-all;
  user-select: text;
}

.qr-item-meta {
  display: flex;
  align-items: center;
  gap: 8px;
}

.qr-transform {
  flex: 1;
  font-size: 11px;
  color: var(--text-secondary);
}

/* 文字识别（OCR）叠框与结果：复用二维码的卡片 / 列表配方，仅用 --primary-color 区分框色。 */
.ocr-overlay {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  pointer-events: none;
}

.ocr-box {
  fill: transparent;
  stroke: var(--primary-color);
  /* viewBox 用原图像素，描边不随缩放变粗。 */
  stroke-width: 2;
  vector-effect: non-scaling-stroke;
  transition: stroke var(--transition-fast), fill var(--transition-fast);
}

/* 可选中文字层（实况文本）：透明文本叠在原图对应位置，只让文字本身吃指针事件、可被拖选；
   ::selection 用半透明高亮，透出底图真实文字。 */
.ocr-text-layer {
  position: absolute;
  inset: 0;
  pointer-events: none;
}

.ocr-text-span {
  position: absolute;
  white-space: pre;
  color: transparent;
  line-height: 1;
  transform-origin: left top;
  cursor: text;
  pointer-events: auto;
  user-select: text;
  -webkit-user-select: text;
}

.ocr-text-span::selection {
  /* 半透明叠层（themes.css 外允许的例外），高亮透出底图文字。 */
  background: color-mix(in srgb, var(--primary-color) 35%, transparent);
}

.ocr-results {
  flex-shrink: 0;
  border: 1px solid var(--border-color);
  background: var(--surface-secondary);
  border-radius: var(--radius-md);
  padding: 10px 12px;
}

.ocr-results-head {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-primary);
}

.ocr-results-title {
  font-size: 12px;
  font-weight: 600;
}

.ocr-spacer {
  flex: 1;
}

.ocr-progress,
.ocr-empty {
  margin-top: 8px;
  font-size: 11px;
  color: var(--text-secondary);
}

/* 识别结果：完整文本直接整块给出（可选中 / 滚动），不再分条列出。 */
.ocr-text-block {
  margin: 8px 0 0;
  padding: 8px 10px;
  max-height: 220px;
  overflow: auto;
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-primary);
  white-space: pre-wrap;
  word-break: break-word;
  user-select: text;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  background: var(--surface-primary);
}

.trunc-note {
  color: var(--sev-warn-fg);
  border: 1px solid var(--sev-warn-border);
  background: var(--sev-warn-bg);
  border-radius: var(--radius-md);
  padding: 6px 10px;
  margin-bottom: 8px;
  font-size: 12px;
}

</style>
