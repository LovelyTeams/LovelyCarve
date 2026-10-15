
import { tr } from '../../../composables/useLocale';
import { ref } from 'vue';
import { save, open as openDialog } from '@tauri-apps/plugin-dialog';
import { readDir } from '@tauri-apps/plugin-fs';
import { fsExport, fsExportDir } from '../../../lib/ipc/fs/export';
import { taskCancel } from '../../../lib/ipc/taskRuntime';
import { onEvent, type UnlistenFn } from '../../../lib/ipc/events';
import { notifyError, notifySuccess, pushPersistentToast } from '../../../composables/useToast';

export type ExportDirMode = 'tree' | 'zip';

export interface ExportableFsNode {
  id: string;
  name: string;
  kind: string;
  size: number;
}

interface UseFileExportsOptions {
  imageId: () => string;
  formatBytes: (bytes: number) => string;
}

interface ExportDirProgress {
  task_id: string;
  files_done: number;
  dirs_done: number;
  bytes_written: number;
  skipped: number;
  skipped_deleted: number;
  current_path: string | null;
}

interface ReadFaultLike {
  bad_regions: Array<{ offset: number; len: number }>;
  synthetic_bytes: number;
  read_faults_detected: boolean;
}

export function useFileExports(options: UseFileExportsOptions) {
  const modePickerOpen = ref(false);
  let modePickerResolver: ((mode: ExportDirMode | null) => void) | null = null;

  function pickExportMode(): Promise<ExportDirMode | null> {
    return new Promise(resolve => {
      modePickerResolver = resolve;
      modePickerOpen.value = true;
    });
  }

  function chooseExportMode(mode: ExportDirMode | null) {
    modePickerOpen.value = false;
    const resolver = modePickerResolver;
    modePickerResolver = null;
    if (resolver) resolver(mode);
  }

  async function exportFile(rowNode: ExportableFsNode | null, checkedFiles: ExportableFsNode[]) {
    if (checkedFiles.length > 1) {
      await exportCheckedFiles(checkedFiles);
      return;
    }
    const single = checkedFiles.length === 1 ? checkedFiles[0] : rowNode;
    if (!single || single.kind !== 'file') return;
    try {
      const dest = await save({
        title: tr("导出文件"),
        defaultPath: single.name,
      });
      if (!dest) return;
      const result = await fsExport({
        imageId: options.imageId(),
        nodeId: single.id,
        destination: dest,
      });
      notifySuccess(
        result.read_faults_detected ? tr("导出成功（含补零）") : tr("导出成功"),
        `${single.name} (${options.formatBytes(result.bytes_written)})${exportFaultDetail(result, options.formatBytes)}`,
      );
    } catch (err) {
      notifyError(tr("导出失败"), String(err));
    }
  }

  async function exportDir(rowNode: ExportableFsNode | null, checkedDirs: ExportableFsNode[]) {
    if (checkedDirs.length > 1) {
      await exportCheckedDirs(checkedDirs);
      return;
    }
    const single = checkedDirs.length === 1 ? checkedDirs[0] : rowNode;
    if (!single || single.kind !== 'dir') return;

    const mode = await pickExportMode();
    if (!mode) return;

    const wrapper = exportFolderName(single.name);
    let target: string;
    if (mode === 'tree') {
      const dest = await openDialog({
        title: tr("选择导出目标目录"),
        directory: true,
        multiple: false,
      });
      if (!dest || typeof dest !== 'string') return;
      // The backend writes children straight into the destination, so wrap
      // them under a folder named after the source directory.
      target = joinExportDest(dest, wrapper);
    } else {
      const dest = await save({
        title: tr("保存为 ZIP 压缩包"),
        defaultPath: `${wrapper}.zip`,
        filters: [{ name: 'ZIP', extensions: ['zip'] }],
      });
      if (!dest) return;
      target = dest.toLowerCase().endsWith('.zip') ? dest : `${dest}.zip`;
    }

    await runExportDir(single, target, mode);
  }

  async function exportCheckedFiles(files: ExportableFsNode[]) {
    const dest = await openDialog({
      title: tr("选择导出目标目录"),
      directory: true,
      multiple: false,
    });
    if (!dest || typeof dest !== 'string') return;

    let cancelled = false;
    const toast = pushPersistentToast({
      kind: 'info',
      title: tr("导出勾选文件中…（{0} 个）", { 0: files.length }),
      detail: tr("{0} · 准备中", { 0: dest }),
      action: {
        get label() { return tr("取消"); },
        onClick: () => {
          cancelled = true;
          toast.update({ action: { get label() { return tr("取消中…"); }, onClick: () => {}, disabled: true } });
        },
      },
    });

    const used = await seedUsedNames(dest);
    let ok = 0;
    let failed = 0;
    let bytes = 0;
    let syntheticBytes = 0;
    let faultRegionHits = 0;
    let lastErr = '';
    for (const file of files) {
      if (cancelled) break;
      const name = dedupeExportName(exportFolderName(file.name), used);
      try {
        const result = await fsExport({
          imageId: options.imageId(),
          nodeId: file.id,
          destination: joinExportDest(dest, name),
        });
        bytes += result.bytes_written;
        if (result.read_faults_detected) {
          syntheticBytes += result.synthetic_bytes;
          faultRegionHits += result.bad_regions.length;
        }
        ok += 1;
      } catch (err) {
        failed += 1;
        lastErr = String(err);
      }
      toast.update({
        detail: `${dest} · ${ok + failed}/${files.length} · ${options.formatBytes(bytes)}${failed > 0 ? tr(" · 失败 {0}", { 0: failed }) : ''}`,
      });
    }

    const parts = [tr("{0} 个文件", { 0: ok }), options.formatBytes(bytes)];
    if (syntheticBytes > 0) {
      parts.push(tr("含补零 {0}（{1} 个坏区命中）", { 0: options.formatBytes(syntheticBytes), 1: faultRegionHits }));
    }
    if (failed > 0) parts.push(tr("失败 {0}（{1}）", { 0: failed, 1: lastErr }));
    if (cancelled) parts.push(tr("未导出 {0}", { 0: files.length - ok - failed }));
    toast.update({
      kind: failed > 0 ? 'error' : cancelled ? 'info' : 'success',
      title: cancelled
        ? tr("导出已取消")
        : failed > 0
          ? tr("导出完成（部分失败）")
          : syntheticBytes > 0
            ? tr("导出成功（含补零）")
            : tr("导出成功"),
      detail: `${dest} · ${parts.join(' · ')}`,
      action: undefined,
    });
    toast.dismiss(failed > 0 ? 6000 : 3200);
  }

  async function exportCheckedDirs(dirs: ExportableFsNode[]) {
    const mode = await pickExportMode();
    if (!mode) return;
    const dest = await openDialog({
      title: tr("选择导出目标目录"),
      directory: true,
      multiple: false,
    });
    if (!dest || typeof dest !== 'string') return;

    const used = await seedUsedNames(dest);
    for (const dir of dirs) {
      const name = dedupeExportName(
        mode === 'zip' ? `${exportFolderName(dir.name)}.zip` : exportFolderName(dir.name),
        used,
      );
      await runExportDir(dir, joinExportDest(dest, name), mode);
    }
  }

  async function runExportDir(node: ExportableFsNode, target: string, mode: ExportDirMode) {
    const taskId = newTaskId();
    const modeLabel = mode === 'zip' ? tr("导出为 ZIP 中…") : tr("导出文件夹中…");
    const toast = pushPersistentToast({
      kind: 'info',
      title: modeLabel,
      detail: tr("{0} · 准备中", { 0: target }),
      action: {
        get label() { return tr("取消"); },
        onClick: () => {
          toast.update({
            detail: tr("{0} · 取消中…", { 0: target }),
            action: { get label() { return tr("取消中…"); }, onClick: () => {}, disabled: true },
          });
          void taskCancel({ taskId }).catch(err => {
            notifyError(tr("取消失败"), String(err));
          });
        },
      },
    });

    let unlisten: UnlistenFn | null = null;
    try {
      unlisten = await onEvent<ExportDirProgress>('fs-export-dir-progress', event => {
        if (event.payload.task_id !== taskId) return;
        const progress = event.payload;
        const parts = [
          tr("{0} 文件", { 0: progress.files_done }),
          tr("{0} 目录", { 0: progress.dirs_done }),
          options.formatBytes(progress.bytes_written),
        ];
        if (progress.skipped > 0) parts.push(tr("跳过 {0}", { 0: progress.skipped }));
        if (progress.skipped_deleted > 0) parts.push(tr("已删除项 {0}", { 0: progress.skipped_deleted }));
        if (progress.current_path) parts.push(tr("当前: {0}", { 0: progress.current_path }));
        toast.update({ detail: `${target} · ${parts.join(' · ')}` });
      });
    } catch (err) {
      notifyError(tr("进度监听失败"), String(err));
    }

    try {
      const summary = await fsExportDir({
        imageId: options.imageId(),
        nodeId: node.id,
        destination: target,
        taskId,
        mode,
      });

      const parts = [
        tr("{0} 个文件", { 0: summary.file_count }),
        tr("{0} 个目录", { 0: summary.dir_count }),
        options.formatBytes(summary.bytes_written),
      ];
      if (summary.skipped_count > 0) parts.push(tr("跳过 {0}", { 0: summary.skipped_count }));
      if (summary.skipped_deleted > 0) parts.push(tr("已删除项 {0}", { 0: summary.skipped_deleted }));
      if (summary.truncated) parts.push(tr("已截断"));
      if (summary.read_faults_detected) {
        parts.push(
          tr("含补零 {0}（{1} 个坏区命中）", { 0: options.formatBytes(summary.synthetic_bytes), 1: summary.bad_regions.length }),
        );
      }

      if (summary.cancelled) {
        toast.update({
          kind: 'info',
          title: tr("导出已取消"),
          detail: `${target} · ${parts.join(' · ')}`,
          action: undefined,
          progress: undefined,
        });
        toast.dismiss(4200);
      } else {
        toast.update({
          kind: 'success',
          title: exportDirDoneTitle(mode, summary.read_faults_detected),
          detail: `${target} · ${parts.join(' · ')}`,
          action: undefined,
          progress: undefined,
        });
        toast.dismiss(3200);
      }
    } catch (err) {
      toast.update({
        kind: 'error',
        title: tr("导出失败"),
        detail: String(err),
        action: undefined,
        progress: undefined,
      });
      toast.dismiss(4200);
    } finally {
      if (unlisten) unlisten();
    }
  }

  async function seedUsedNames(dest: string): Promise<Set<string>> {
    const used = new Set<string>();
    try {
      for (const entry of await readDir(dest)) used.add(entry.name.toLowerCase());
    } catch (err) {
      notifyError(tr("无法枚举目标目录"), tr("重名检测退化为批内去重：{0}", { 0: String(err) }));
    }
    return used;
  }

  return {
    modePickerOpen,
    chooseExportMode,
    exportFile,
    exportDir,
  };
}

function dedupeExportName(name: string, used: Set<string>): string {
  if (!used.has(name.toLowerCase())) {
    used.add(name.toLowerCase());
    return name;
  }
  const dot = name.lastIndexOf('.');
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : '';
  for (let index = 2; ; index += 1) {
    const candidate = `${stem} (${index})${ext}`;
    if (!used.has(candidate.toLowerCase())) {
      used.add(candidate.toLowerCase());
      return candidate;
    }
  }
}

function joinExportDest(dir: string, name: string): string {
  const sep = dir.includes('\\') && !dir.includes('/') ? '\\' : '/';
  const base = dir.endsWith(sep) ? dir.slice(0, -sep.length) : dir;
  return `${base}${sep}${name}`;
}

function exportFaultDetail(report: ReadFaultLike, formatBytes: (bytes: number) => string): string {
  if (!report.read_faults_detected) {
    return '';
  }
  return tr(" · 警告：含 {0} 合成零字节（{1} 个坏区命中）", { 0: formatBytes(report.synthetic_bytes), 1: report.bad_regions.length });
}

function exportDirDoneTitle(mode: ExportDirMode, readFaultsDetected: boolean): string {
  if (mode === 'zip') {
    return readFaultsDetected ? tr("ZIP 压缩完成（含补零）") : tr("ZIP 压缩完成");
  }
  return readFaultsDetected ? tr("目录导出完成（含补零）") : tr("目录导出完成");
}

function exportFolderName(raw: string): string {
  let out = '';
  for (const char of raw) {
    const code = char.codePointAt(0) ?? 0;
    out += '<>:"/\\|?*'.includes(char) || code < 0x20 ? '_' : char;
  }
  const trimmed = out.replace(/[\s.]+$/, '');
  return trimmed.length === 0 ? '_' : trimmed;
}

function newTaskId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `fs-task-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}
