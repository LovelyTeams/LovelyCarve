import { invokeCmd } from './client';

export interface SystemPathInfo {
  exists: boolean;
  is_file: boolean;
  is_directory: boolean;
  is_symlink: boolean;
}

export function systemPathInfo(path: string): Promise<SystemPathInfo> {
  return invokeCmd<SystemPathInfo>('system_path_info', { path });
}

export interface ReleaseUpdateInfo {
  available: boolean;
  version: string;
  signed: boolean;
  canInstall: boolean;
}

export function releaseUpdateCheck(): Promise<ReleaseUpdateInfo> {
  return invokeCmd<ReleaseUpdateInfo>('release_update_check');
}

export function releaseUpdateInstall(version: string): Promise<void> {
  return invokeCmd<void>('release_update_install', { version });
}
