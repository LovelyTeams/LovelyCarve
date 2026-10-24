import type { ObjectDirective } from 'vue';
import { animateUi, cancelUiAnimation, useUiMotion } from './useUiMotion';

/** Animate a changed tab/section without replacing its cached component.
 *  The plain form is a light fade for high-frequency changes (stepping through
 *  files); `v-content-motion.rise` fades the new view up for page and tab switches. */
export const vContentMotion: ObjectDirective<HTMLElement, unknown> = {
  updated(element, binding) {
    if (binding.value === binding.oldValue || !element.getClientRects().length) return;
    if (binding.modifiers.rise) {
      animateUi(element, [{ opacity: 0, translate: '0 10px' }, { opacity: 1, translate: '0 0' }], undefined, { speed: 'normal' });
    } else {
      animateUi(element, [{ opacity: 0.65 }, { opacity: 1 }]);
    }
  },
  beforeUnmount: cancelUiAnimation,
};

const disclosures = new WeakMap<HTMLDetailsElement, { target?: boolean; click: (event: MouseEvent) => void }>();

/** Keep native summary keyboard behavior and animate both open and close. */
export const vDisclosureMotion: ObjectDirective<HTMLDetailsElement> = {
  beforeUpdate(element, _binding, vnode, previous) {
    // A reactive open prop takes precedence over an in-flight user toggle.
    if (vnode.props?.open !== previous?.props?.open) cancelUiAnimation(element);
  },
  mounted(element) {
    const state = { target: undefined as boolean | undefined, click: (_event: MouseEvent) => {} };
    state.click = event => {
      if (event.defaultPrevented || event.button !== 0 || !(event.target instanceof Element)) return;
      const summary = event.target.closest('summary');
      if (summary?.parentElement !== element) return;
      if (event.target.closest('a, button, input, select, textarea, label')) return;
      event.preventDefault();
      const expanded = !(state.target ?? element.open);
      const startHeight = element.getBoundingClientRect().height;
      cancelUiAnimation(element);
      if (!useUiMotion().motionEnabled.value || !element.animate) {
        element.open = expanded;
        return;
      }
      element.open = expanded;
      const endHeight = element.getBoundingClientRect().height;
      element.open = true;
      state.target = expanded;
      // Menus may position their contents outside the details box. Do not clip
      // those popovers when the expanded and collapsed heights are identical.
      if (Math.abs(startHeight - endHeight) < 1) {
        animateUi(element, [{ opacity: expanded ? 0.65 : 1 }, { opacity: expanded ? 1 : 0.65 }], () => {
          element.open = expanded;
          state.target = undefined;
        });
        return;
      }
      const overflow = element.style.overflow;
      element.style.overflow = 'hidden';
      animateUi(element, [{ height: `${startHeight}px` }, { height: `${endHeight}px` }], () => {
        element.style.overflow = overflow;
        element.open = expanded;
        state.target = undefined;
      }, { speed: 'normal', easing: 'expand' });
    };
    disclosures.set(element, state);
    element.addEventListener('click', state.click);
  },
  beforeUnmount(element) {
    cancelUiAnimation(element);
    const state = disclosures.get(element);
    if (state) element.removeEventListener('click', state.click);
    disclosures.delete(element);
  },
};
