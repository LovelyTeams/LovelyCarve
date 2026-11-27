import { inject, type InjectionKey } from 'vue';

export const readonlyMountSupportKey: InjectionKey<boolean> = Symbol('readonlyMountSupport');

export function useReadonlyMountSupport(): boolean {
  return inject(readonlyMountSupportKey, false);
}
