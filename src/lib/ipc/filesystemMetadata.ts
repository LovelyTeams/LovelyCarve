import { invokeCmd } from './client';
import type { ImageId } from './ids';



/** 某个卷在源 Windows 系统上的原始盘符（C:、D:…）。`partition_id` 对应目录树
 *  根节点 id `part:N`。 */
export interface DriveLetterAssignment {
  partition_id: number;
  /** 形如 "C:"，始终带尾冒号。 */
  drive_letter: string;
}

export interface DriveLetterReport {
  assignments: DriveLetterAssignment[];
  /** 多个 Windows 安装给出冲突盘符（双系统/重装）时为 true。 */
  ambiguous: boolean;
}

/** 解析 Windows 源系统给各卷分配的原始盘符（`HKLM\SYSTEM\MountedDevices`）。
 *  读 SYSTEM 注册表配置单元（10–40 MiB）并关联到分区，后端 blocking 执行；
 *  非 Windows / 上锁 / 无 MountedDevices 时返回空 assignments。 */
export function winDriveLetters(args: { imageId: ImageId }): Promise<DriveLetterReport> {
  return invokeCmd<DriveLetterReport>('win_drive_letters', args);
}
