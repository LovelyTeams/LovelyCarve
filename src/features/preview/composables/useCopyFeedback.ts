import { onScopeDispose, ref } from 'vue';
import { tr } from '../../../composables/useLocale';
import { notifyError } from '../../../composables/useToast';

/** Copy buttons that show a check mark for 1.5 s after copying (docs/FRONTEND.md §7). */
export function useCopyFeedback() {
  const copiedKey = ref<string | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let request = 0;

  async function copy(key: string, text: string) {
    const current = ++request;
    try {
      await navigator.clipboard.writeText(text);
      if (current !== request) return;
      clearTimeout(timer);
      copiedKey.value = key;
      timer = setTimeout(() => { copiedKey.value = null; }, 1500);
    } catch (error) {
      if (current === request) notifyError(tr('复制失败'), String(error));
    }
  }

  function reset() {
    request++;
    clearTimeout(timer);
    copiedKey.value = null;
  }

  onScopeDispose(reset);
  return { copiedKey, copy, reset };
}
