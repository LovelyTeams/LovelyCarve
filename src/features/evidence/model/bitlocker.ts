import { sourceText, type Translator } from '../../../i18n/translate';
export type BitlockerUnlockMode = 'password' | 'recovery' | 'vmk' | 'key';

export interface BitlockerUnlockJob {
  imageId: string;
  partitionId: number;
  taskId: string;
  mode: BitlockerUnlockMode;
  bytesDone: number;
  bytesTotal: number;
  cancelling: boolean;
}

export interface BitlockerScanJob {
  imageId: string;
  partitionId: number;
  taskId: string;
  bytesDone: number;
  bytesTotal: number;
  cancelling: boolean;
}

export interface BitlockerVmkCandidate {
  offset: number;
  type_flag: number;
  key_type: number;
  vmk_hex: string;
  recovery_key: string | null;
  already_tried: boolean;
}

export interface BitlockerVmkScanResult {
  partition_id: number;
  candidates: BitlockerVmkCandidate[];
}

export type BitlockerInputMode = 'password' | 'recovery' | 'memory' | 'key';

export type BitlockerKeyCredential =
  | { kind: 'clear' }
  | { kind: 'startup'; startupKeyPath: string }
  | { kind: 'vmk'; vmkHex: string }
  | { kind: 'fvek'; fvekHex: string; tweakHex: string };

function formatBitlockerBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(2)} ${units[i]}`;
}

/** UI projection shared by unlock and memory scan jobs; does not own task state. */
export function bitlockerProgress(job: { bytesDone: number; bytesTotal: number } | undefined, tr: Translator = sourceText) {
  const percent = !job || job.bytesTotal === 0 ? 0 : Math.min(100, (job.bytesDone / job.bytesTotal) * 100);
  return { percent, percentText: `${percent.toFixed(1)}%`, bytesText: !job ? '' : job.bytesTotal === 0 ? tr("准备中…") : `${formatBitlockerBytes(job.bytesDone)} / ${formatBitlockerBytes(job.bytesTotal)}` };
}
