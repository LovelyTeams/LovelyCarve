import { createApp, type Component } from "vue";
import "./styles/global.css";

import { bootstrapWindow } from "./app/bootstrap";
import { readonlyMountSupportKey } from "./composables/useReadonlyMountSupport";
import { fsMountSupported } from "./lib/ipc/mount";

bootstrapWindow();

// Multi-window dispatch. Tauri loads the same index.html in every window;
// the detached file preview passes `?view=file-preview` so the main app's
// chrome (sidebar, status bar, tab manager) is never constructed there. Both
// roots are dynamic imports so neither window pulls in the other's bundle.
const view = new URLSearchParams(window.location.search).get("view");

async function mount() {
  const detached = view === "file-preview";
  const mountSupported = detached ? Promise.resolve(false) : fsMountSupported().catch(() => false);
  const root: Component = detached
    ? (await import("./views/FilePreviewWindow.vue")).default
    : (await import("./App.vue")).default;
  createApp(root).provide(readonlyMountSupportKey, await mountSupported).mount("#app");
}

void mount();
