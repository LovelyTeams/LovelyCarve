import { tr } from './useLocale';
import { notifyError, notifySuccess } from './useToast';

/** Writes `text` to the clipboard and reports the outcome in a toast;
 *  `what` describes what was copied ("3 条完整路径"). */
export async function copyText(text: string, what: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    notifySuccess(tr("已复制"), what);
    return true;
  } catch (error) {
    notifyError(tr("复制失败"), String(error));
    return false;
  }
}

/** Copies the text `pending` resolves to, such as a link the backend makes;
 *  failing to make it is reported like a failed copy. */
export async function copyPending(pending: Promise<string>, what: string): Promise<boolean> {
  let text: string;
  try {
    text = await pending;
  } catch (error) {
    notifyError(tr("复制失败"), String(error));
    return false;
  }
  return copyText(text, what);
}
