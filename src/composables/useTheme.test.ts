// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let startViewTransition: ReturnType<typeof vi.fn>;
let animate: ReturnType<typeof vi.fn>;
let dispose: (() => void) | undefined;

beforeEach(() => {
  localStorage.clear();
  vi.resetModules();
  vi.stubGlobal('matchMedia', vi.fn(() => Object.assign(new EventTarget(), { matches: false })));
  startViewTransition = vi.fn((update: () => void) => {
    update();
    return { ready: Promise.resolve(), finished: Promise.resolve(), updateCallbackDone: Promise.resolve() };
  });
  Object.defineProperty(document, 'startViewTransition', { configurable: true, value: startViewTransition });
  animate = vi.fn();
  Object.defineProperty(document.documentElement, 'animate', { configurable: true, value: animate });
});

afterEach(() => {
  dispose?.();
  dispose = undefined;
  Reflect.deleteProperty(document, 'startViewTransition');
  Reflect.deleteProperty(document.documentElement, 'animate');
  vi.unstubAllGlobals();
});

async function loadTheme() {
  const { useTheme } = await import('./useTheme');
  const { initUiMotion } = await import('./useUiMotion');
  dispose = initUiMotion();
  const theme = useTheme();
  theme.initTheme();
  return theme;
}

function controlAt(x: number, y: number): HTMLButtonElement {
  const control = document.createElement('button');
  vi.spyOn(control, 'getBoundingClientRect').mockReturnValue({ left: x - 10, top: y - 10, width: 20, height: 20 } as DOMRect);
  return control;
}

describe('theme switch', () => {
  it('reveals the new theme from the clicked control inside a view transition', async () => {
    const theme = await loadTheme();
    theme.setTheme('dark', controlAt(100, 40));
    expect(startViewTransition).toHaveBeenCalledOnce();
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(localStorage.getItem('lovelycarve-theme')).toBe('dark');
    await vi.waitFor(() => expect(animate).toHaveBeenCalledOnce());
    const [frames, options] = animate.mock.calls[0];
    expect(frames.clipPath[0]).toBe('circle(0px at 100px 40px)');
    expect(options).toMatchObject({ duration: 400, pseudoElement: '::view-transition-new(root)' });
  });

  it('applies at once with motion off, without a control, or for the current theme', async () => {
    localStorage.setItem('lovelycarve-ui-animations', '"off"');
    const theme = await loadTheme();
    theme.setTheme('dark', controlAt(10, 10));
    expect(document.documentElement.dataset.theme).toBe('dark');
    const { motionPreference } = (await import('./useUiPreferences')).useUiPreferences();
    motionPreference.value = 'system';
    theme.setTheme('light');
    expect(document.documentElement.dataset.theme).toBe('light');
    theme.setTheme('light', controlAt(10, 10));
    expect(startViewTransition).not.toHaveBeenCalled();
    expect(localStorage.getItem('lovelycarve-theme')).toBe('light');
  });

  it('follows the system appearance when the stored theme is not one this build ships', async () => {
    localStorage.setItem('lovelycarve-theme', 'pink');
    vi.stubGlobal('matchMedia', vi.fn((query: string) => Object.assign(new EventTarget(), { matches: query === '(prefers-color-scheme: dark)' })));
    const theme = await loadTheme();
    expect(theme.currentTheme.value).toBe('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
  });
});
