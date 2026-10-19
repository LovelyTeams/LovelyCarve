// Directory reads below a Docker container (`host-docker:<id>:…`) first merge
// the container's layers into a browsable index (`fs_docker_prepare`; the
// merge is a task the task center lists). Concurrent reads of one container
// share one merge; a case switch meanwhile abandons the read.
import { tr } from '../../../composables/useLocale';
import { fsChildren, fsChildrenPage, fsDockerPrepare, fsFlatPage } from '../../../lib/ipc/fs/query';
import { useCaseSession } from '../../cases/api';

const preparations = new Map<string, Promise<void>>();

function caseDirectory(): string | null {
  return useCaseSession().activeCase.value?.dir ?? null;
}

async function afterMerge<T>(args: { imageId: string; parentId: string | null }, read: () => Promise<T>): Promise<T> {
  const root = args.parentId?.match(/^host-docker:[^:]+:/)?.[0];
  if (!root) return read();
  const caseDir = caseDirectory();
  const key = `${caseDir ?? ''}\u001f${args.imageId}\u001f${root}`;
  let preparation = preparations.get(key);
  if (!preparation) {
    preparation = (async () => {
      const request = { imageId: args.imageId, rootId: root, caseDir };
      const cached = await fsDockerPrepare({ ...request, mode: 'status' });
      if (cached.ready) return;
      if (caseDir !== caseDirectory()) throw new Error(tr("案件已切换，请重新打开容器"));
      const built = await fsDockerPrepare({ ...request, mode: 'build' });
      if (!built.ready) throw new Error(tr("容器索引尚未完成，请重试"));
    })();
    preparations.set(key, preparation);
  }
  try {
    await preparation;
    if (caseDir !== caseDirectory()) throw new Error(tr("案件已切换，请重新打开容器"));
    return await read();
  } finally {
    if (preparations.get(key) === preparation) preparations.delete(key);
  }
}

export function containerFsChildrenPage(args: Parameters<typeof fsChildrenPage>[0]) {
  return afterMerge(args, () => fsChildrenPage(args));
}
export function containerFsChildren(args: Parameters<typeof fsChildren>[0]) {
  return afterMerge(args, () => fsChildren(args));
}
export function containerFsFlatPage(args: Parameters<typeof fsFlatPage>[0]) {
  return afterMerge(args, () => fsFlatPage(args));
}
