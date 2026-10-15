
import { tr } from '../../../composables/useLocale';
import { computed, nextTick, onScopeDispose, ref, watch } from 'vue';
import { svgPoints, type ImageRecognitionView, type OcrTextItem } from '../model/imageRecognition';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { ocrScan, type OcrOutcome } from '../../../lib/ipc/ocr';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';
import { qrScan, type QrScanOutcome, type QrScanProgress } from '../../../lib/ipc/qrcode';
import { notifyError } from '../../../composables/useToast';

interface UseImageRecognitionOptions {
  imageId: () => string;
  nodeId: () => string;
  category: () => string;
}

function newTaskId(prefix: string): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) return crypto.randomUUID();
  return `${prefix}-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

export function useImageRecognition(options: UseImageRecognitionOptions) {
  // Explicit DOM bindings let the display component own the actual nodes.
  const imageRef = ref<HTMLImageElement | null>(null);
  const imageNatural = ref<{ w: number; h: number } | null>(null);
  const imageDisplay = ref<{ w: number; h: number } | null>(null);
  let imageResizeObserver: ResizeObserver | null = null;

  const qrScanning = ref(false);
  const qrResult = ref<QrScanOutcome | null>(null);
  const qrError = ref<string | null>(null);
  const qrProgress = ref<{ attempts: number; total: number; found: number } | null>(null);
  const qrExpected = ref(1);
  const qrActiveIndex = ref<number | null>(null);
  const qrCopiedIndex = ref<number | null>(null);
  let qrOwner: { taskId: string; unlisten: UnlistenFn | null; started: boolean } | null = null;
  let generation = 0;
  let disposed = false;
  let qrCopyTimer: number | null = null;
  let ocrCopyTimer: number | null = null;

  const ocrScanning = ref(false);
  const ocrResult = ref<OcrOutcome | null>(null);
  const ocrError = ref<string | null>(null);
  const ocrCopiedAll = ref(false);
  const ocrTextLayer = ref<HTMLElement | null>(null);

  const ocrFullText = computed(() => ocrResult.value?.regions.map((region) => region.text).join('\n') ?? '');

  const displayScale = computed(() => {
    const natural = imageNatural.value;
    const display = imageDisplay.value;
    return natural && display && natural.w > 0 ? display.w / natural.w : 1;
  });

  const ocrTextItems = computed<OcrTextItem[]>(() =>
    (ocrResult.value?.regions ?? []).map((region) => {
      const xs = region.points.map((point) => point[0]);
      const ys = region.points.map((point) => point[1]);
      const minX = Math.min(...xs);
      const minY = Math.min(...ys);
      return {
        text: region.text,
        minX,
        minY,
        w: Math.max(...xs) - minX,
        h: Math.max(...ys) - minY,
      };
    }),
  );

  function updateImageDisplay() {
    const element = imageRef.value;
    if (element) imageDisplay.value = { w: element.clientWidth, h: element.clientHeight };
  }

  function fitOcrText() {
    const layer = ocrTextLayer.value;
    if (!layer) return;
    const spans = layer.querySelectorAll<HTMLElement>('.ocr-text-span');
    const items = ocrTextItems.value;
    const scale = displayScale.value;
    spans.forEach((span, index) => {
      const item = items[index];
      if (!item) return;
      span.style.transform = 'scaleX(1)';
      const natural = span.scrollWidth;
      const target = item.w * scale;
      span.style.transform = `scaleX(${natural > 0 ? target / natural : 1})`;
    });
  }

  function observeImageSize() {
    if (imageResizeObserver || !imageRef.value || typeof ResizeObserver === 'undefined') return;
    imageResizeObserver = new ResizeObserver(() => {
      if (disposed) return;
      updateImageDisplay();
      scheduleFit();
    });
    imageResizeObserver.observe(imageRef.value);
  }

  function onImageLoad() {
    if (disposed) return;
    const element = imageRef.value;
    if (!element) return;
    imageNatural.value = { w: element.naturalWidth, h: element.naturalHeight };
    updateImageDisplay();
    observeImageSize();
  }

  function resetQr() {
    qrResult.value = null;
    qrError.value = null;
    qrProgress.value = null;
    qrActiveIndex.value = null;
    qrCopiedIndex.value = null;
  }

  function resetOcr() {
    ocrResult.value = null;
    ocrError.value = null;
    ocrCopiedAll.value = false;
  }

  function resetImageRecognition() {
    generation++;
    const owner = qrOwner;
    qrOwner = null;
    if (owner?.unlisten) { owner.unlisten(); owner.unlisten = null; }
    if (owner?.started) void taskCancel({ taskId: owner.taskId }).catch(error => console.warn('取消旧二维码扫描失败', error));
    qrScanning.value = false;
    ocrScanning.value = false;
    if (qrCopyTimer !== null) window.clearTimeout(qrCopyTimer);
    if (ocrCopyTimer !== null) window.clearTimeout(ocrCopyTimer);
    qrCopyTimer = null; ocrCopyTimer = null;
    imageResizeObserver?.disconnect(); imageResizeObserver = null;
    resetQr();
    resetOcr();
    imageNatural.value = null;
    imageDisplay.value = null;
  }

  async function runQrScan() {
    if (disposed || qrScanning.value || options.category() !== 'image') return;
    resetQr();
    qrScanning.value = true;
    const taskId = newTaskId('qr');
    const owner = { taskId, unlisten: null as UnlistenFn | null, started: false };
    qrOwner = owner;
    const requestGeneration = generation;
    const imageId = options.imageId();
    const nodeId = options.nodeId();
    const expectedCount = qrExpected.value;
    const current = () => !disposed && requestGeneration === generation && qrOwner === owner
      && imageId === options.imageId() && nodeId === options.nodeId();
    try {
      owner.unlisten = await onEvent<QrScanProgress>('qr-scan-progress', (event) => {
        if (!current() || event.payload.task_id !== taskId) return;
        qrProgress.value = {
          attempts: event.payload.attempts,
          total: event.payload.total,
          found: event.payload.found,
        };
      });
      if (!current()) return;
      owner.started = true;
      const result = await qrScan({
        imageId,
        nodeId,
        expectedCount,
        taskId,
      });
      if (!current()) return;
      qrResult.value = result;
      if (result.codes.length === 0 && !result.cancelled) qrError.value = tr("未识别到二维码");
    } catch (error) {
      if (current()) qrError.value = String(error);
    } finally {
      if (owner.unlisten) {
        owner.unlisten();
        owner.unlisten = null;
      }
      if (current()) {
        qrScanning.value = false;
        qrProgress.value = null;
      }
      if (qrOwner === owner) qrOwner = null;
    }
  }

  async function cancelQrScan() {
    if (qrOwner?.started) await taskCancel({ taskId: qrOwner.taskId }).catch(error => notifyError(tr("取消二维码扫描失败"), String(error)));
  }

  async function copyQrText(text: string, index: number) {
    const requestGeneration = generation;
    try {
      await navigator.clipboard.writeText(text);
      if (disposed || requestGeneration !== generation) return;
      qrCopiedIndex.value = index;
      if (qrCopyTimer !== null) window.clearTimeout(qrCopyTimer);
      qrCopyTimer = window.setTimeout(() => {
        qrCopyTimer = null;
        if (qrCopiedIndex.value === index) qrCopiedIndex.value = null;
      }, 1500);
    } catch {
      if (!disposed && requestGeneration === generation) notifyError(tr("复制失败"));
    }
  }

  async function runOcrScan() {
    if (disposed || ocrScanning.value || options.category() !== 'image') return;
    resetOcr();
    ocrScanning.value = true;
    const nodeId = options.nodeId();
    const imageId = options.imageId(), requestGeneration = generation;
    const current = () => !disposed && requestGeneration === generation && imageId === options.imageId() && nodeId === options.nodeId();
    try {
      const result = await ocrScan({ imageId, nodeId });
      if (!current()) return;
      ocrResult.value = result;
      if (result.regions.length === 0) ocrError.value = tr("未识别到文字");
    } catch (error) {
      if (current()) ocrError.value = String(error);
    } finally {
      if (current()) ocrScanning.value = false;
    }
  }

  async function copyAllOcrText() {
    if (!ocrFullText.value) return;
    const requestGeneration = generation;
    try {
      await navigator.clipboard.writeText(ocrFullText.value);
      if (disposed || requestGeneration !== generation) return;
      ocrCopiedAll.value = true;
      if (ocrCopyTimer !== null) window.clearTimeout(ocrCopyTimer);
      ocrCopyTimer = window.setTimeout(() => {
        ocrCopyTimer = null;
        ocrCopiedAll.value = false;
      }, 1500);
    } catch {
      if (!disposed && requestGeneration === generation) notifyError(tr("复制失败"));
    }
  }

  function ocrSpanStyle(item: OcrTextItem) {
    const scale = displayScale.value;
    return {
      left: `${item.minX * scale}px`,
      top: `${item.minY * scale}px`,
      fontSize: `${Math.max(1, item.h * scale * 0.82)}px`,
    };
  }

  function disposeImageRecognition() {
    if (disposed) return;
    disposed = true;
    resetImageRecognition();
  }
  onScopeDispose(disposeImageRecognition);

  function scheduleFit() {
    const requestGeneration = generation;
    void nextTick(() => { if (!disposed && requestGeneration === generation) fitOcrText(); });
  }
  watch([ocrTextItems, imageDisplay], scheduleFit);

  const view = computed<ImageRecognitionView>(() => ({
    imageNatural: imageNatural.value,
    qrScanning: qrScanning.value, qrResult: qrResult.value, qrError: qrError.value,
    qrProgress: qrProgress.value, qrCopiedIndex: qrCopiedIndex.value,
    ocrScanning: ocrScanning.value, ocrResult: ocrResult.value, ocrError: ocrError.value,
    ocrCopiedAll: ocrCopiedAll.value, ocrFullText: ocrFullText.value,
    ocrTextItems: ocrTextItems.value.map(item => ({ text: item.text, style: ocrSpanStyle(item) })),
  }));

  return {
    view,
    elements: { image: imageRef, textLayer: ocrTextLayer },
    imageNatural,
    imageDisplay,
    qrScanning,
    qrResult,
    qrError,
    qrProgress,
    qrExpected,
    qrActiveIndex,
    qrCopiedIndex,
    ocrScanning,
    ocrResult,
    ocrError,
    ocrCopiedAll,
    ocrFullText,
    ocrTextItems,
    onImageLoad,
    resetImageRecognition,
    qrSvgPoints: svgPoints,
    runQrScan,
    cancelQrScan,
    copyQrText,
    ocrSvgPoints: svgPoints,
    runOcrScan,
    copyAllOcrText,
    ocrSpanStyle,
    disposeImageRecognition,
  };
}
