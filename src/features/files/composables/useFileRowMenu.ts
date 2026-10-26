import { onActivated, onDeactivated, onMounted, onScopeDispose, type Ref } from 'vue';
import { useContextMenu } from '../../../composables/useContextMenu';
import { isHostDockerMountRoot, rowKeyOf, type DiskFsNode } from '../model/model';
import type { FileMenuScope } from './useFileActions';

export interface FileRowMenuTarget {
  node: DiskFsNode;
  scope: FileMenuScope;
}

interface MenuPoint {
  x: number;
  y: number;
}

/** Owns the file list's context menus (row, blank area, column header) and the
 *  visible panel's Escape precedence. */
export function useFileRowMenu(options: {
  selectedKey: Ref<string | null>;
  previewVisible: Ref<boolean>;
  isChecked: (id: string) => boolean;
  filterVisible: () => boolean;
  closeFilter: () => void;
  exportModeOpen: () => boolean;
  cancelExportMode: () => void;
  hasChecked: () => boolean;
  clearChecked: () => void;
  refreshMounts: () => Promise<void>;
}) {
  const row = useContextMenu<FileRowMenuTarget>();
  const list = useContextMenu<null>();
  /** The column key of the header cell the menu opened on (null: the check column). */
  const header = useContextMenu<string | null>();
  let active = true;
  let disposed = false;

  function hideMenus() {
    row.close();
    list.close();
    header.close();
  }

  const anyMenuOpen = () => row.menu.open || list.menu.open || header.menu.open;

  /** Opens the row menu at a viewport point and moves the cursor to the row.
   *  Opened on a checked row, its batch actions act on every checked row;
   *  on any other row, on that row alone. */
  function showRowMenu(point: MenuPoint, node: DiskFsNode) {
    if (!active || disposed) return;
    options.closeFilter();
    row.openAt(point.x, point.y, { node, scope: options.isChecked(node.id) ? 'checked' : 'row' });
    options.selectedKey.value = rowKeyOf(node);
    if (isHostDockerMountRoot(node)) void options.refreshMounts();
  }

  function onRowContextMenu(event: MouseEvent, node: DiskFsNode) {
    event.preventDefault();
    showRowMenu({ x: event.clientX, y: event.clientY }, node);
  }

  /** The menu for the current directory, opened on the list's blank area. */
  function showListMenu(point: MenuPoint) {
    if (!active || disposed) return;
    options.closeFilter();
    list.openAt(point.x, point.y, null);
  }

  function showHeaderMenu(point: MenuPoint, columnKey: string | null) {
    if (!active || disposed) return;
    options.closeFilter();
    header.openAt(point.x, point.y, columnKey);
  }

  // An open menu holds the focus and handles its own keys; this covers the
  // layers below it.
  function onKeydown(event: KeyboardEvent) {
    if (event.isComposing || event.keyCode === 229 || event.key !== 'Escape') return;
    if (anyMenuOpen()) hideMenus();
    else if (options.filterVisible()) options.closeFilter();
    else if (options.exportModeOpen()) options.cancelExportMode();
    else {
      const target = event.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) return;
      if (options.previewVisible.value) options.previewVisible.value = false;
      else if (options.hasChecked()) options.clearChecked();
    }
  }

  function activate() {
    active = true;
    document.addEventListener('keydown', onKeydown);
  }
  function deactivate() {
    active = false;
    document.removeEventListener('keydown', onKeydown);
    hideMenus();
  }
  onMounted(activate);
  onActivated(activate);
  onDeactivated(deactivate);
  onScopeDispose(() => { disposed = true; deactivate(); });
  return {
    rowMenu: row.menu, listMenu: list.menu, headerMenu: header.menu,
    anyMenuOpen, hideMenus, showRowMenu, onRowContextMenu, showListMenu, showHeaderMenu,
  };
}
