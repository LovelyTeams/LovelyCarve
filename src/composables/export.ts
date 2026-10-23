
import { tr } from './useLocale';
// File-save helpers that pair a native dialog + write with a toast on
// completion. These orchestrate Tauri plugin I/O *and* user-facing
// notification (useToast), so they live at the composable layer — depending
// on useToast here is legal, whereas it was a reverse dependency back when
// this file sat under utils/.
import { save } from '@tauri-apps/plugin-dialog';
import { writeFile } from '@tauri-apps/plugin-fs';
import { notifyError, notifyExportSuccess } from './useToast';

type SaveFilter = { name: string; extensions: string[] };

interface SaveTextOptions {
  defaultPath: string;
  content: string;
  filters: SaveFilter[];
  title?: string;
  successLabel?: string;
}

export async function saveTextWithDialog(options: SaveTextOptions) {
  try {
    const path = await save({
      title: options.title,
      defaultPath: options.defaultPath,
      filters: options.filters,
    });
    if (!path) return null;
    await writeFile(path, new TextEncoder().encode(options.content));
    notifyExportSuccess(path, options.successLabel);
    return path;
  } catch (error) {
    notifyError(tr("导出失败"), String(error));
    return null;
  }
}
