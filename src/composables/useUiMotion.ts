import { computed, ref, watch } from 'vue';
import { parseMotionPreference, UI_ANIMATIONS_KEY, useUiPreferences } from './useUiPreferences';

/** Duration token: `--motion-fast` / `--motion-normal` / `--motion-slow`. */
export type MotionSpeed = 'fast' | 'normal' | 'slow';
/** Easing token: `--ease-out` (entrances), `--ease-in` (exits), `--ease-spring` (small pops),
 *  `--ease-settle` (dialogs and popovers landing), `--ease-snap` (sliding selection
 *  indicators) and `--ease-expand` (size reveals). */
export type MotionEasing = 'out' | 'in' | 'spring' | 'settle' | 'snap' | 'expand';

export interface UiAnimationOptions {
  speed?: MotionSpeed;
  easing?: MotionEasing;
}

const { motionPreference } = useUiPreferences();
const systemReducedMotion = ref(false);
const motionEnabled = computed(() => motionPreference.value === 'on'
  || (motionPreference.value === 'system' && !systemReducedMotion.value));
let cleanup: (() => void) | null = null;
const activeAnimations = new Map<HTMLElement, () => void>();
const fallbackDurations: Record<MotionSpeed, number> = { fast: 150, normal: 250, slow: 400 };

/** Resolve CSS time tokens to the milliseconds WAAPI expects, including minified seconds. */
export function motionTiming(element: Element, speed: MotionSpeed = 'fast', easing: MotionEasing = 'out'): { duration: number; easing: string } {
  const styles = getComputedStyle(element);
  // Production CSS may shorten 250ms to .25s; parseFloat alone loses the unit.
  const time = styles.getPropertyValue(`--motion-${speed}`).trim().match(/^([+]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)(ms|s)$/i);
  const duration = time ? Number(time[1]) * (time[2]?.toLowerCase() === 's' ? 1000 : 1) : NaN;
  return {
    duration: Number.isFinite(duration) ? duration : fallbackDurations[speed],
    easing: styles.getPropertyValue(`--ease-${easing}`).trim() || 'ease-out',
  };
}

/** Finish the current interaction before a reversal, unmount, or preference change. */
export function cancelUiAnimation(element: HTMLElement): void {
  activeAnimations.get(element)?.();
}

/** One animation per surface; completion always releases temporary layout styles. */
export function animateUi(
  element: HTMLElement,
  frames: Keyframe[],
  complete: () => void = () => {},
  options: UiAnimationOptions = {},
): void {
  cancelUiAnimation(element);
  if (!motionEnabled.value || !element.animate) {
    complete();
    return;
  }
  const animation = element.animate(frames, motionTiming(element, options.speed, options.easing));
  let finished = false;
  const finish = () => {
    if (finished) return;
    finished = true;
    activeAnimations.delete(element);
    animation.onfinish = null;
    animation.oncancel = null;
    animation.cancel();
    complete();
  };
  activeAnimations.set(element, finish);
  animation.onfinish = finish;
  animation.oncancel = finish;
  // WAAPI rejects `finished` on cancellation. Reversals, unmounts and motion
  // preference changes are expected cancellations and share the same cleanup.
  void animation.finished?.then(finish, finish);
}

/** Flip the effective state with the least forcing preference: switching motion
 *  on only overrides the OS when the OS is the one asking for reduced motion. */
export function setMotionEnabled(enabled: boolean): void {
  if (!enabled) motionPreference.value = 'off';
  else motionPreference.value = systemReducedMotion.value ? 'on' : 'system';
}

watch(motionEnabled, enabled => {
  if (!enabled) [...activeAnimations.values()].forEach(finish => finish());
}, { flush: 'sync' });

function readStoredValue(raw: string | null): unknown {
  if (raw === null) return null;
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

/** Apply preferences before mounting each webview; synchronize detached windows too. */
export function initUiMotion(): () => void {
  if (cleanup) return cleanup;
  const media = window.matchMedia?.('(prefers-reduced-motion: reduce)');
  systemReducedMotion.value = media?.matches ?? false;
  const onSystemChange = (event: MediaQueryListEvent) => {
    systemReducedMotion.value = event.matches;
  };
  const onStorage = (event: StorageEvent) => {
    if (event.storageArea !== localStorage || (event.key !== UI_ANIMATIONS_KEY && event.key !== null)) return;
    motionPreference.value = parseMotionPreference(readStoredValue(event.newValue));
  };
  const stop = watch(motionEnabled, enabled => {
    document.documentElement.dataset.motion = enabled ? 'full' : 'reduced';
  }, { immediate: true, flush: 'sync' });
  media?.addEventListener('change', onSystemChange);
  window.addEventListener('storage', onStorage);
  cleanup = () => {
    [...activeAnimations.values()].forEach(finish => finish());
    stop();
    media?.removeEventListener('change', onSystemChange);
    window.removeEventListener('storage', onStorage);
    cleanup = null;
  };
  return cleanup;
}

export function useUiMotion() {
  return { motionEnabled, systemReducedMotion, setMotionEnabled };
}
