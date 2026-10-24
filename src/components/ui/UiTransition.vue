<template>
  <Transition :css="false" @enter="enter" @leave="leave" @enter-cancelled="cancel" @leave-cancelled="cancel"><slot /></Transition>
</template>

<script setup lang="ts">
import { onBeforeUnmount } from 'vue';
import { animateUi, cancelUiAnimation, type UiAnimationOptions } from '../../composables/useUiMotion';

/** collapse / horizontal: height or width to zero (inline panels, detail panes);
 *  fade: opacity only (overlays); pop: fade plus a small scale (popovers);
 *  dock: a right-hand detail dock that opens within the current window.
 *  Page switches and detail visibility never change native window bounds. */
const props = withDefaults(defineProps<{ kind?: 'collapse' | 'horizontal' | 'fade' | 'pop' | 'dock' }>(), { kind: 'collapse' });
// Entrances take the longer, decelerating curve; exits get out of the way quickly.
// Popovers land on the settle spring; size reveals open on the expand curve.
const enterTiming: UiAnimationOptions = { speed: 'normal', easing: 'out' };
const leaveTiming: UiAnimationOptions = { speed: 'fast', easing: 'in' };
const popEnterTiming: UiAnimationOptions = { speed: 'slow', easing: 'settle' };
const activeElements = new Set<HTMLElement>();
onBeforeUnmount(() => {
  for (const element of activeElements) {
    cancel(element);
  }
});
function cancel(element: Element) {
  const target = element as HTMLElement;
  activeElements.delete(target);
  cancelUiAnimation(target);
}
function transition(element: Element, done: () => void, entering: boolean) {
  const target = element as HTMLElement;
  cancelUiAnimation(target);
  activeElements.add(target);
  const finish = () => {
    activeElements.delete(target);
    done();
  };
  const timing = entering ? enterTiming : leaveTiming;
  if (props.kind === 'fade') {
    animateUi(target, [{ opacity: entering ? 0 : 1 }, { opacity: entering ? 1 : 0 }], finish, timing);
    return;
  }
  if (props.kind === 'pop') {
    const hidden: Keyframe = { opacity: 0, scale: '0.95', translate: '0 -4px' };
    const shown: Keyframe = { opacity: 1, scale: '1', translate: '0 0' };
    animateUi(target, entering ? [hidden, shown] : [shown, hidden], finish, entering ? popEnterTiming : timing);
    return;
  }
  resize(target, finish, entering, props.kind === 'horizontal' || props.kind === 'dock' ? 'width' : 'height');
}
/** Grow from, or shrink to, zero along one axis. */
function resize(target: HTMLElement, finish: () => void, entering: boolean, dimension: 'width' | 'height') {
  const horizontal = dimension === 'width';
  const minimum = horizontal ? 'minWidth' : 'minHeight';
  const size = target.getBoundingClientRect()[dimension];
  const styles = getComputedStyle(target);
  const expanded: Keyframe = { [dimension]: `${size}px`, opacity: 1 };
  const collapsed: Keyframe = { [dimension]: '0px', opacity: 0 };
  // Padding, borders and margins must shrink too, otherwise the final removal
  // leaves a visible jump even after the content height has reached zero.
  const spacing = horizontal
    ? ['paddingLeft', 'paddingRight', 'marginLeft', 'marginRight', 'borderLeftWidth', 'borderRightWidth'] as const
    : ['paddingTop', 'paddingBottom', 'marginTop', 'marginBottom', 'borderTopWidth', 'borderBottomWidth'] as const;
  for (const property of spacing) {
    if (styles[property] === 'auto') continue;
    expanded[property] = styles[property];
    collapsed[property] = '0px';
  }
  const overflow = target.style.overflow;
  const previousMinimum = target.style[minimum];
  target.style.overflow = 'hidden';
  target.style[minimum] = '0';
  animateUi(target, entering ? [collapsed, expanded] : [expanded, collapsed], () => {
    target.style.overflow = overflow;
    target.style[minimum] = previousMinimum;
    finish();
  }, { speed: (entering ? enterTiming : leaveTiming).speed, easing: entering ? 'expand' : 'out' });
}
function enter(element: Element, done: () => void) { transition(element, done, true); }
function leave(element: Element, done: () => void) { transition(element, done, false); }
</script>
