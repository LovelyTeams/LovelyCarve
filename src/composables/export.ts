
import { tr } from './useLocale';
// File-save helpers that pair a native dialog + write with a toast on
// completion. These orchestrate Tauri plugin I/O *and* user-facing
// notification (useToast), so they live at the composable layer — depending
// on useToast here is legal, whereas it was a reverse dependency back when
// this file sat under utils/.
import { open, save } from '@tauri-apps/plugin-dialog';
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

interface SaveBinaryOptions {
  defaultPath: string;
  bytes: Uint8Array;
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

export async function saveBinaryWithDialog(options: SaveBinaryOptions) {
  try {
    const path = await save({
      title: options.title,
      defaultPath: options.defaultPath,
      filters: options.filters,
    });
    if (!path) return null;
    await writeFile(path, options.bytes);
    notifyExportSuccess(path, options.successLabel);
    return path;
  } catch (error) {
    notifyError(tr("导出失败"), String(error));
    return null;
  }
}

export interface BatchFileEntry {
  filename: string;
  bytes: Uint8Array;
}

interface SaveBatchToFolderOptions {
  title?: string;
  files: BatchFileEntry[];
  /// Optional summary text written alongside the files (e.g. manifest).
  summaryFilename?: string;
  summaryContent?: string;
  /// Toast label on success; receives the count of files actually written.
  successLabel?: (count: number, folder: string) => string;
}

/// Ask the user for a destination *folder* once, then dump every file in
/// `files` into it. Handles duplicate filenames by suffixing `_1`, `_2`…
/// Sanitises filenames so they can't escape the chosen directory.
/// Returns the folder path on success, `null` if the user cancelled.
export async function saveBatchToFolder(options: SaveBatchToFolderOptions) {
  try {
    const folder = await open({
      title: options.title ?? tr("选择导出目录"),
      directory: true,
      multiple: false,
    });
    if (!folder || typeof folder !== 'string') return null;

    // Normalise path separator — writeFile accepts forward slashes on
    // Windows too, and joining this way avoids a platform-specific util.
    const sep = folder.includes('\\') && !folder.includes('/') ? '\\' : '/';
    const base = folder.endsWith(sep) ? folder.slice(0, -sep.length) : folder;

    const seen = new Set<string>();
    const uniqueName = (raw: string): string => {
      // Strip path separators to keep everything inside the chosen folder.
      const safe = raw.replace(/[\\/]/g, '_').replace(/^\.+/, '') || 'file';
      if (!seen.has(safe)) {
        seen.add(safe);
        return safe;
      }
      const dot = safe.lastIndexOf('.');
      const stem = dot > 0 ? safe.slice(0, dot) : safe;
      const ext = dot > 0 ? safe.slice(dot) : '';
      for (let i = 1; i < 10000; i++) {
        const candidate = `${stem}_${i}${ext}`;
        if (!seen.has(candidate)) {
          seen.add(candidate);
          return candidate;
        }
      }
      // Extremely unlikely collision storm; fall through with timestamp.
      const fallback = `${stem}_${Date.now()}${ext}`;
      seen.add(fallback);
      return fallback;
    };

    let written = 0;
    for (const f of options.files) {
      const name = uniqueName(f.filename);
      try {
        await writeFile(`${base}${sep}${name}`, f.bytes);
        written += 1;
      } catch (e) {
        // Keep going so one bad filename doesn't wipe out the batch.
        console.error(`写入 ${name} 失败:`, e);
      }
    }

    if (options.summaryContent && options.summaryFilename) {
      const name = uniqueName(options.summaryFilename);
      try {
        await writeFile(
          `${base}${sep}${name}`,
          new TextEncoder().encode(options.summaryContent),
        );
      } catch (e) {
        console.error('写入清单失败:', e);
      }
    }

    notifyExportSuccess(
      base,
      options.successLabel?.(written, base) ?? tr("已导出 {0} 个文件到 {1}", { 0: written, 1: base }),
    );
    return base;
  } catch (error) {
    notifyError(tr("批量导出失败"), String(error));
    return null;
  }
}
