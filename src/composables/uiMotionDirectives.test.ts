// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createApp, h, nextTick, ref, vShow, withDirectives, type App } from 'vue';
import UiTransition from '../components/ui/UiTransition.vue';
import { vContentMotion, vDisclosureMotion } from './uiMotionDirectives';
import { initUiMotion } from './useUiMotion';
import { useUiPreferences } from './useUiPreferences';

type MockAnimation = { onfinish: (() => void) | null; oncancel: (() => void) | null; cancel: ReturnType<typeof vi.fn> };
let app: App | undefined;
let host: HTMLDivElement;
let dispose: () => void;
let animations: MockAnimation[];
const originalAnimate = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'animate');
const { motionPreference } = useUiPreferences();

beforeEach(() => {
  motionPreference.value = 'system';
  vi.stubGlobal('matchMedia', () => Object.assign(new EventTarget(), { matches: false }));
  dispose = initUiMotion();
  animations = [];
  Object.defineProperty(HTMLElement.prototype, 'animate', { configurable: true, value: vi.fn(() => {
    const animation: MockAnimation = { onfinish: null, oncancel: null, cancel: vi.fn() };
    animations.push(animation);
    return animation;
  }) });
  host = document.createElement('div');
  document.body.append(host);
});

afterEach(() => {
  app?.unmount();
  app = undefined;
  host.remove();
  dispose();
  motionPreference.value = 'system';
  if (originalAnimate) Object.defineProperty(HTMLElement.prototype, 'animate', originalAnimate);
  else Reflect.deleteProperty(HTMLElement.prototype, 'animate');
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function mountDisclosure(controlledOpen?: { value: boolean }) {
  app = createApp({ render: () => withDirectives(h('details', controlledOpen ? { open: controlledOpen.value } : {}, [
    h('summary', ['详情', h('button', '操作')]),
    h('p', '内容'),
    withDirectives(h('details', [h('summary', '嵌套详情'), h('p', '嵌套内容')]), [[vDisclosureMotion]]),
  ]), [[vDisclosureMotion]]) });
  app.mount(host);
  const details = host.querySelector('details')!;
  vi.spyOn(details, 'getBoundingClientRect').mockImplementation(() => ({ height: details.open ? 160 : 32 }) as DOMRect);
  return details;
}

describe('disclosure motion', () => {
  it('finishes at the last requested state after rapid open/close/open clicks', () => {
    const details = mountDisclosure();
    const summary = details.querySelector('summary')!;
    summary.click();
    summary.click();
    summary.click();
    expect(animations).toHaveLength(3);
    expect(animations[0].cancel).toHaveBeenCalledOnce();
    expect(animations[1].cancel).toHaveBeenCalledOnce();
    animations[2].onfinish?.();
    expect(details.open).toBe(true);
    expect(details.style.overflow).toBe('');
  });

  it('settles a closing disclosure immediately when animations are disabled', () => {
    const details = mountDisclosure();
    details.open = true;
    details.querySelector('summary')!.click();
    expect(details.open).toBe(true); // Body remains visible during its exit.
    motionPreference.value = 'off';
    expect(details.open).toBe(false);
    expect(details.style.overflow).toBe('');
    details.querySelector('summary')!.click();
    expect(details.open).toBe(true);
    expect(animations).toHaveLength(1);
  });

  it('lets a reactive open prop override a pending close', async () => {
    const open = ref(false);
    const details = mountDisclosure(open);
    details.open = true;
    details.querySelector('summary')!.click();
    open.value = true;
    await nextTick();
    expect(details.open).toBe(true);
    expect(details.style.overflow).toBe('');
    expect(animations[0].cancel).toHaveBeenCalledOnce();
  });

  it('does not toggle parents for nested summaries or embedded buttons', () => {
    const details = mountDisclosure();
    // Preserve native control behavior, including the DOM emulator's default action.
    const nativeDetails = document.createElement('details');
    nativeDetails.innerHTML = '<summary><button>操作</button></summary>';
    nativeDetails.querySelector('button')!.click();
    details.querySelector('button')!.click();
    expect(animations).toHaveLength(0);
    expect(details.open).toBe(nativeDetails.open);
    details.open = true;
    details.querySelector('details')!.querySelector('summary')!.click();
    expect(animations).toHaveLength(1);
    expect(details.style.overflow).toBe('');
  });

  it('does not clip an absolutely positioned menu with unchanged height', () => {
    const details = mountDisclosure();
    vi.mocked(details.getBoundingClientRect).mockReturnValue({ height: 32 } as DOMRect);
    details.querySelector('summary')!.click();
    expect(details.style.overflow).toBe('');
    animations[0].onfinish?.();
    expect(details.open).toBe(true);
  });

  it('releases pending animations when the parent is removed', () => {
    const details = mountDisclosure();
    details.querySelector('summary')!.click();
    app!.unmount();
    app = undefined;
    expect(animations[0].cancel).toHaveBeenCalledOnce();
    expect(details.style.overflow).toBe('');
    details.querySelector('summary')!.click();
    expect(animations).toHaveLength(1);
  });
});

describe('conditional panel motion', () => {
  it('preserves v-show state and releases temporary layout styles on reversal and disable', async () => {
    const visible = ref(false);
    app = createApp({ render: () => h(UiTransition, { kind: 'horizontal' }, {
      default: () => withDirectives(h('aside', { style: { minWidth: '200px', overflow: 'auto' } }, [h('input')]), [[vShow, visible.value]]),
    }) });
    app.mount(host);
    const panel = host.querySelector('aside')!;
    const input = panel.querySelector('input')!;
    input.value = '保留输入';
    visible.value = true;
    await nextTick();
    visible.value = false;
    await nextTick();
    visible.value = true;
    await nextTick();
    motionPreference.value = 'off';
    expect(panel.style.display).not.toBe('none');
    expect(panel.style.minWidth).toBe('200px');
    expect(panel.style.overflow).toBe('auto');
    expect(input.value).toBe('保留输入');
    visible.value = false;
    await nextTick();
    expect(panel.style.display).toBe('none');
  });

  it('removes an exiting v-if panel when motion is disabled mid-animation', async () => {
    const visible = ref(true);
    app = createApp({ render: () => h(UiTransition, {}, { default: () => visible.value ? h('section', '详情') : null }) });
    app.mount(host);
    visible.value = false;
    await nextTick();
    expect(host.querySelector('section')).not.toBeNull();
    motionPreference.value = 'off';
    expect(host.querySelector('section')).toBeNull();
  });

  it('fades changed tabs without remounting their state and cancels on disable', async () => {
    const tab = ref('tasks');
    app = createApp({ render: () => withDirectives(h('main', [h('input')]), [[vContentMotion, tab.value]]) });
    app.mount(host);
    const panel = host.querySelector('main')!;
    vi.spyOn(panel, 'getClientRects').mockReturnValue([{}] as DOMRectList);
    const input = panel.querySelector('input')!;
    input.value = '保留输入';
    tab.value = 'quick';
    await nextTick();
    expect(animations).toHaveLength(1);
    expect(panel.querySelector('input')).toBe(input);
    expect(input.value).toBe('保留输入');
    motionPreference.value = 'off';
    expect(animations[0].cancel).toHaveBeenCalledOnce();
  });

  it('keeps a light fade by default and rises page switches with .rise', async () => {
    const tab = ref('files');
    app = createApp({ render: () => h('div', [
      withDirectives(h('main', { id: 'light' }), [[vContentMotion, tab.value]]),
      withDirectives(h('main', { id: 'page' }), [[vContentMotion, tab.value, undefined, { rise: true }]]),
    ]) });
    app.mount(host);
    for (const panel of host.querySelectorAll('main')) {
      vi.spyOn(panel, 'getClientRects').mockReturnValue([{}] as DOMRectList);
    }
    tab.value = 'search';
    await nextTick();
    const calls = vi.mocked(HTMLElement.prototype.animate).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0][0]).toEqual([{ opacity: 0.65 }, { opacity: 1 }]);
    expect(calls[1][0]).toEqual([{ opacity: 0, translate: '0 10px' }, { opacity: 1, translate: '0 0' }]);
    // Without theme tokens the helper falls back to the documented defaults.
    expect(calls[0][1]).toEqual({ duration: 150, easing: 'ease-out' });
    expect(calls[1][1]).toEqual({ duration: 250, easing: 'ease-out' });
  });

  it('pops popovers in and out and removes them once the exit finishes', async () => {
    const visible = ref(false);
    app = createApp({ render: () => h(UiTransition, { kind: 'pop' }, { default: () => visible.value ? h('div', '菜单') : null }) });
    app.mount(host);
    visible.value = true;
    await nextTick();
    const [frames, timing] = vi.mocked(HTMLElement.prototype.animate).mock.calls[0] as [Keyframe[], KeyframeAnimationOptions];
    expect(frames[0]).toMatchObject({ opacity: 0, scale: '0.95' });
    // The entrance lands on the settle spring over the slow duration (defaults without theme tokens).
    expect(timing).toEqual({ duration: 400, easing: 'ease-out' });
    animations[0].onfinish?.();
    visible.value = false;
    await nextTick();
    expect(host.querySelector('div')).not.toBeNull();
    animations[1].onfinish?.();
    expect(host.querySelector('div')).toBeNull();
  });
});
