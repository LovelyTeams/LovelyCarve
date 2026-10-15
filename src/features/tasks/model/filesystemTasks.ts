import { sourceText, type Translator } from '../../../i18n/translate';
import type { BuiltinTask } from './types';
import type { SnapshotProgress } from '../../../lib/ipc/cases';
import type { BackendTaskSnapshot } from '../../../lib/ipc/taskRuntime';

export function isSnapshotTask(task: BackendTaskSnapshot) {
  return task.kind === 'volumeSnapshot' || task.kind === 'sessionSnapshot';
}

function stageTask(kind: 'filesystem' | 'index', taskId: string, imageId: string, volumeKey: string, now: number, tr: Translator = sourceText): BuiltinTask {
  const partition = volumeKey.match(/^part:(\d+)$/)?.[1];
  return {
    id: `${kind}:${taskId}`, kind, imageId,
    title: kind === 'filesystem' ? tr("文件系统加载") : tr("索引建立"),
    subject: partition !== undefined ? tr("分区 {0}", { 0: partition }) : volumeKey === 'root' ? tr("根目录") : volumeKey, trigger: 'manually',
    status: 'queued', progress: null, message: kind === 'index' ? tr("等待文件系统加载") : tr("等待加载"),
    error: null, started: now, finished: null, cancelId: taskId, cancelling: false, results: 0, timelineRows: 0,
  };
}

/** Two displayed stages share one backend cancellation token. */
export function applySnapshotTasks(previous: Map<string, BuiltinTask>, event: SnapshotProgress, now: number, tr: Translator = sourceText) {
  if (!event.task_id || event.phase === 'reused') return previous;
  const tasks = new Map(previous);
  const filesystemId = `filesystem:${event.task_id}`;
  const indexId = `index:${event.task_id}`;
  const filesystem = { ...(tasks.get(filesystemId) ?? stageTask('filesystem', event.task_id, event.image_id, event.volume_key, now, tr)) };
  const index = { ...(tasks.get(indexId) ?? stageTask('index', event.task_id, event.image_id, event.volume_key, now, tr)) };
  const terminal = (task: BuiltinTask) => ['success', 'failed', 'cancel'].includes(task.status);
  const complete = (task: BuiltinTask, message: string) => {
    if (task.status === 'failed' || task.status === 'cancel') return;
    Object.assign(task, { status: 'success', progress: 100, finished: task.finished ?? now, cancelling: false, error: null, results: Math.max(task.results, event.entries), message });
  };
  const suffix = event.truncated ? tr(" · 已达条目上限，结果不完整") : '';
  if (event.phase === 'started') {
    Object.assign(filesystem, stageTask('filesystem', event.task_id, event.image_id, event.volume_key, now, tr), { status: 'running', message: tr("正在读取文件系统") });
    Object.assign(index, stageTask('index', event.task_id, event.image_id, event.volume_key, now, tr));
  } else if (event.phase === 'walking') {
    if (terminal(filesystem)) return previous;
    Object.assign(filesystem, { status: 'running', results: event.entries, message: tr("已读取 {0} 项{1}", { 0: event.entries.toLocaleString(), 1: suffix }) });
  } else if (event.phase === 'indexing') {
    complete(filesystem, tr("目录读取完成{0}", { 0: suffix }));
    if (!terminal(index)) Object.assign(index, {
      status: 'running', error: null, finished: null, results: event.entries,
      message: event.message || tr("已写入 {0} 项，正在建立索引", { 0: event.entries.toLocaleString() }),
    });
  } else if (event.phase === 'done') {
    complete(filesystem, tr("已加载 {0} 项{1}", { 0: event.entries.toLocaleString(), 1: suffix }));
    complete(index, `${event.message || tr("索引已就绪 · {0} 项", { 0: event.entries.toLocaleString() })}${suffix}`);
  } else {
    const target = filesystem.status === 'success' ? index : filesystem;
    if (!terminal(target)) Object.assign(target, {
      status: event.phase === 'cancelled' ? 'cancel' : 'failed', finished: now, cancelling: false,
      error: event.phase === 'error' ? event.message || tr("文件系统或索引处理失败") : null,
      message: event.message || (event.phase === 'cancelled' ? tr("已取消") : tr("处理失败")),
    });
    if (target === filesystem && !terminal(index)) Object.assign(index, { status: 'cancel', finished: now, cancelling: false, message: tr("文件系统未就绪，索引未执行") });
  }
  tasks.set(filesystemId, filesystem);
  tasks.set(indexId, index);
  return tasks;
}

/** Recover queued/running work when the page attaches after the initial event. */
export function recoverSnapshotTasks(previous: Map<string, BuiltinTask>, native: BackendTaskSnapshot, now: number, restart = false, tr: Translator = sourceText) {
  if (!native.imageId) return previous;
  const tasks = new Map(previous);
  const volumeKey = native.id.split(`${native.imageId}:`)[1] || tr("当前文件系统");
  for (const kind of ['filesystem', 'index'] as const) {
    const id = `${kind}:${native.id}`;
    const current = tasks.get(id);
    if (current && !restart) {
      if (['queued', 'running'].includes(current.status)) tasks.set(id, { ...current,
        status: kind === 'filesystem' && native.status === 'running' ? 'running' : current.status,
        message: kind === 'filesystem' && current.status === 'queued' && native.status === 'running' ? tr("正在读取文件系统") : current.message,
        cancelling: native.status === 'cancellationRequested',
      });
      continue;
    }
    tasks.set(id, { ...stageTask(kind, native.id, native.imageId, volumeKey, now - native.elapsedMs, tr),
      status: kind === 'filesystem' && native.status !== 'queued' ? 'running' : 'queued',
      message: kind === 'filesystem' && native.status !== 'queued' ? tr("正在读取文件系统") : kind === 'index' ? tr("等待文件系统加载") : tr("等待加载"),
      cancelling: native.status === 'cancellationRequested',
    });
  }
  return tasks;
}
