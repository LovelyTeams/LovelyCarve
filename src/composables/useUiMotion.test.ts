// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { nextTick } from 'vue';

let dispose: (() => void) | undefined;
let media: EventTarget & { matches: boolean };

beforeEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.motion;
  vi.resetModules();
  media = Object.assign(new EventTarget(), { matches: false });
  vi.stubGlobal('matchMedia', vi.fn(() => media));
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  vi.unstubAllGlobals();
});

function systemChange(matches: boolean) {
  media.matches = matches;
  media.dispatchEvent(Object.assign(new Event('change'), { matches }));
}

describe('UI motion preferences', () => {
  it('applies the saved off preference before mounting', async () => {
    localStorage.setItem('lovelycarve-ui-animations', '"off"');
    const { initUiMotion, useUiMotion } = await import('./useUiMotion');
    dispose = initUiMotion();
    expect(useUiMotion().motionEnabled.value).toBe(false);
    expect(document.documentElement.dataset.motion).toBe('reduced');
  });

  it.each([
    ['false', 'off'],
    ['true', 'system'],
    ['"on"', 'on'],
    ['not json', 'system'],
  ])('reads the stored value %s as %s and rewrites the old boolean', async (stored, expected) => {
    localStorage.setItem('lovelycarve-ui-animations', stored);
    const { useUiPreferences } = await import('./useUiPreferences');
    expect(useUiPreferences().motionPreference.value).toBe(expected);
    await nextTick();
    if (stored === 'false' || stored === 'true') {
      expect(localStorage.getItem('lovelycarve-ui-animations')).toBe(JSON.stringify(expected));
    }
  });

  it('updates the document immediately and persists changes', async () => {
    const { initUiMotion } = await import('./useUiMotion');
    const { motionPreference } = (await import('./useUiPreferences')).useUiPreferences();
    dispose = initUiMotion();
    expect(document.documentElement.dataset.motion).toBe('full');
    motionPreference.value = 'off';
    expect(document.documentElement.dataset.motion).toBe('reduced');
    await nextTick();
    expect(localStorage.getItem('lovelycarve-ui-animations')).toBe('"off"');
    motionPreference.value = 'system';
    expect(document.documentElement.dataset.motion).toBe('full');
    await nextTick();
    expect(localStorage.getItem('lovelycarve-ui-animations')).toBe('"system"');
  });

  it('follows the system setting only in 跟随系统 and lets 开启 override it', async () => {
    media.matches = true;
    const { initUiMotion } = await import('./useUiMotion');
    const { motionPreference } = (await import('./useUiPreferences')).useUiPreferences();
    dispose = initUiMotion();
    expect(motionPreference.value).toBe('system');
    expect(document.documentElement.dataset.motion).toBe('reduced');
    motionPreference.value = 'on';
    expect(document.documentElement.dataset.motion).toBe('full');
    systemChange(false);
    systemChange(true);
    expect(document.documentElement.dataset.motion).toBe('full');
    motionPreference.value = 'system';
    expect(document.documentElement.dataset.motion).toBe('reduced');
    systemChange(false);
    expect(document.documentElement.dataset.motion).toBe('full');
    motionPreference.value = 'off';
    systemChange(true);
    systemChange(false);
    expect(document.documentElement.dataset.motion).toBe('reduced');
  });

  it('switches on with the least forcing preference', async () => {
    const { initUiMotion, useUiMotion } = await import('./useUiMotion');
    const { motionPreference } = (await import('./useUiPreferences')).useUiPreferences();
    const { setMotionEnabled, motionEnabled } = useUiMotion();
    dispose = initUiMotion();
    setMotionEnabled(false);
    expect(motionPreference.value).toBe('off');
    setMotionEnabled(true);
    expect(motionPreference.value).toBe('system');
    systemChange(true);
    expect(motionEnabled.value).toBe(false);
    setMotionEnabled(true);
    expect(motionPreference.value).toBe('on');
    expect(motionEnabled.value).toBe(true);
  });

  it('synchronizes other windows and resets to default when storage is cleared', async () => {
    const { initUiMotion } = await import('./useUiMotion');
    dispose = initUiMotion();
    window.dispatchEvent(new StorageEvent('storage', {
      key: 'lovelycarve-ui-animations', newValue: '"off"', storageArea: localStorage,
    }));
    expect(document.documentElement.dataset.motion).toBe('reduced');
    window.dispatchEvent(new StorageEvent('storage', {
      key: 'unrelated', newValue: '"on"', storageArea: localStorage,
    }));
    expect(document.documentElement.dataset.motion).toBe('reduced');
    window.dispatchEvent(new StorageEvent('storage', { key: null, storageArea: localStorage }));
    expect(document.documentElement.dataset.motion).toBe('full');
    // A window still running the earlier build writes the old boolean.
    window.dispatchEvent(new StorageEvent('storage', {
      key: 'lovelycarve-ui-animations', newValue: 'false', storageArea: localStorage,
    }));
    expect(document.documentElement.dataset.motion).toBe('reduced');
  });

  it('initializes only once and releases its listeners on disposal', async () => {
    const { initUiMotion } = await import('./useUiMotion');
    dispose = initUiMotion();
    expect(initUiMotion()).toBe(dispose);
    dispose();
    systemChange(true);
    expect(document.documentElement.dataset.motion).toBe('full');
  });
});

describe('motion tokens', () => {
  it.each([
    ['250ms', 250],
    ['.25s', 250],
    ['.15s', 150],
    ['.4s', 400],
    ['0s', 0],
    ['2.5e-1s', 250],
    ['invalid', 250],
    ['-1ms', 250],
  ])('converts the CSS duration %s to %s milliseconds for native animations', async (token, duration) => {
    const { animateUi } = await import('./useUiMotion');
    const element = document.createElement('div');
    document.body.append(element);
    element.style.setProperty('--motion-normal', token);
    const animate = vi.spyOn(element, 'animate');

    animateUi(element, [{ opacity: 0 }, { opacity: 1 }], undefined, { speed: 'normal' });

    expect(animate).toHaveBeenCalledWith([{ opacity: 0 }, { opacity: 1 }], { duration, easing: 'ease-out' });
    element.remove();
  });

  it('completes a canceled native animation once without an unhandled rejection', async () => {
    const { animateUi, cancelUiAnimation } = await import('./useUiMotion');
    const element = document.createElement('div');
    document.body.append(element);
    const animate = vi.spyOn(element, 'animate');
    const complete = vi.fn();
    animateUi(element, [{ opacity: 0 }, { opacity: 1 }], complete);
    expect(animate).toHaveBeenCalledOnce();
    cancelUiAnimation(element);
    cancelUiAnimation(element);
    await nextTick();
    expect(complete).toHaveBeenCalledTimes(1);
    element.remove();
  });

  it('reads durations and curves from the theme tokens on the element', async () => {
    const { motionTiming } = await import('./useUiMotion');
    const element = document.createElement('div');
    document.body.append(element);
    element.style.setProperty('--motion-slow', '420ms');
    element.style.setProperty('--ease-spring', 'cubic-bezier(0.3, 1.4, 0.6, 1)');
    expect(motionTiming(element, 'slow', 'spring')).toEqual({ duration: 420, easing: 'cubic-bezier(0.3, 1.4, 0.6, 1)' });
    expect(motionTiming(element, 'normal', 'in')).toEqual({ duration: 250, easing: 'ease-out' });
    element.remove();
  });
});
