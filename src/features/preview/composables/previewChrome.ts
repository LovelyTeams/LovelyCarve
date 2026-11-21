import { inject, provide, type InjectionKey, type Ref } from 'vue';

/** Where a viewer may place controls in the chrome of the preview that hosts it:
 *  `tools` joins the view bar beside the view switcher, `statusStart` and
 *  `statusEnd` the two ends of the status bar under the content. */
export type PreviewChromeArea = 'tools' | 'statusStart' | 'statusEnd';

/** One preview instance's chrome targets. The elements belong to that
 *  instance, so viewers kept mounted in hidden tabs never draw into the
 *  visible one. */
export type PreviewChrome = Record<PreviewChromeArea, Readonly<Ref<HTMLElement | null>>>;

const PREVIEW_CHROME: InjectionKey<PreviewChrome> = Symbol('preview-chrome');

export function providePreviewChrome(chrome: PreviewChrome) {
  provide(PREVIEW_CHROME, chrome);
}

/** The hosting preview's chrome, or null when a viewer renders on its own. */
export function usePreviewChrome(): PreviewChrome | null {
  return inject(PREVIEW_CHROME, null);
}
