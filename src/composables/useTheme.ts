import { ref } from 'vue';
import { motionTiming, useUiMotion } from './useUiMotion';

export const themes = ['light', 'dark', 'pink'] as const;
export type ThemeName = typeof themes[number];

const THEME_KEY = 'lovelycarve-theme';
const currentTheme = ref<ThemeName>('light');
let initialized = false;

function isThemeName(value: string | null): value is ThemeName {
  return !!value && themes.includes(value as ThemeName);
}

function detectSystemTheme(): ThemeName {
  if (typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: dark)').matches) {
    return 'dark';
  }
  return 'light';
}

function applyTheme(theme: ThemeName) {
  currentTheme.value = theme;
  if (typeof document !== 'undefined') {
    document.documentElement.setAttribute('data-theme', theme);
  }
}

function initTheme() {
  if (initialized) return;
  initialized = true;

  let saved: string | null = null;
  try {
    saved = localStorage.getItem(THEME_KEY);
  } catch { /* localStorage may be unavailable in restricted contexts */ }

  applyTheme(isThemeName(saved) ? saved : detectSystemTheme());
}

/**
 * Switch and persist the theme. Pass the control that was clicked (`from`) to
 * reveal the new theme as a circle growing out of it; that runs inside a view
 * transition, so it is skipped with motion off or where the webview has no
 * `startViewTransition`, and the theme then applies at once.
 */
function setTheme(theme: ThemeName, from?: Element | null) {
  const commit = () => {
    applyTheme(theme);
    try {
      localStorage.setItem(THEME_KEY, theme);
    } catch { /* localStorage may be unavailable in restricted contexts */ }
  };
  if (!from || theme === currentTheme.value || !useUiMotion().motionEnabled.value
    || typeof document.startViewTransition !== 'function') {
    commit();
    return;
  }
  const rect = from.getBoundingClientRect();
  const x = rect.left + rect.width / 2;
  const y = rect.top + rect.height / 2;
  const radius = Math.hypot(Math.max(x, window.innerWidth - x), Math.max(y, window.innerHeight - y));
  document.startViewTransition(commit).ready.then(() => {
    document.documentElement.animate(
      { clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${radius}px at ${x}px ${y}px)`] },
      { ...motionTiming(document.documentElement, 'slow', 'out'), pseudoElement: '::view-transition-new(root)' },
    );
  }, () => { /* Transition skipped (e.g. hidden window): the theme is already applied. */ });
}

export function useTheme() {
  return {
    currentTheme,
    initTheme,
    setTheme,
    themes,
  };
}
