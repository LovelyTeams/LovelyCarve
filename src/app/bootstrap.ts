import { installConsoleCapture } from '../composables/useConsoleCapture';
import { useTheme } from '../composables/useTheme';
import { initUiMotion } from '../composables/useUiMotion';
import { installIndicatorMotion } from '../composables/indicatorMotion';
import { initLocale } from '../composables/useLocale';

/** Apply window preferences before either root mounts. */
export function bootstrapWindow() {
  // Apply the persisted (or system) theme before anything renders, so dark and
  // pink users do not see the light defaults fade in on every launch.
  useTheme().initTheme();

  initUiMotion();
  // Segmented thumbs and tab underlines slide between items (motion on only).
  installIndicatorMotion();
  initLocale();

  // Capture startup output before mounting; settings can inspect it later.
  installConsoleCapture();

}
