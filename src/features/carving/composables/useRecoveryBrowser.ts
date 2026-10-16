import { computed, ref, shallowRef, watch } from 'vue';
import { tr } from '../../../composables/useLocale';
import type { CarveArtifact, CarveCategory, RecoveryScan } from '../../../lib/ipc/carve';
import {
  artifactEntry, buildFilesystemTree, entryCategory, entryMatches, groupArtifacts, isRecoverable,
  RECOVERY_ROOT, SIGNATURE_ROOT, type RecoveryEntry,
} from '../model/recoveryTree';

export interface RecoveryTreeRow { entry: RecoveryEntry; depth: number; expandable: boolean }

/** File-list style browsing over one scan: volumes and signature results under a virtual root. */
export function useRecoveryBrowser(options: {
  /** Navigation, selection and filters reset when this changes. */
  context: () => string | null;
  filesystem: () => RecoveryScan | null;
  artifacts: () => readonly CarveArtifact[];
  /** Keeps the signature folder visible while a carve has found nothing yet. */
  signatures: () => boolean;
  formatLabel: (formatId: string) => string;
}) {
  const currentKey = ref(RECOVERY_ROOT);
  const selectedKey = ref<string | null>(null);
  const checkedKeys = shallowRef(new Set<string>());
  const expanded = shallowRef(new Set<string>());
  const query = ref('');
  const queryDraft = ref('');
  const categoryFilter = ref<'all' | CarveCategory>('all');
  const categoryFilterDraft = ref<'all' | CarveCategory>('all');

  const fsTree = computed(() => buildFilesystemTree(options.filesystem()));
  const groups = computed(() => groupArtifacts(options.artifacts(), options.formatLabel, options.signatures(), tr));
  const hasFilters = computed(() => query.value.trim().length > 0 || categoryFilter.value !== 'all');

  function lookup(key: string | null): RecoveryEntry | null {
    if (!key || key === RECOVERY_ROOT) return null;
    if (key === SIGNATURE_ROOT) return groups.value.root;
    if (key.startsWith(`${SIGNATURE_ROOT}:`)) return groups.value.formats.find(item => item.key === key) ?? null;
    if (key.startsWith('a:')) {
      const artifact = groups.value.byId.get(key.slice(2));
      return artifact ? artifactEntry(artifact, options.formatLabel) : null;
    }
    return fsTree.value.entries.get(key) ?? null;
  }
  function childrenOf(key: string): RecoveryEntry[] {
    if (key === RECOVERY_ROOT) return [...fsTree.value.roots, ...(groups.value.root ? [groups.value.root] : [])];
    if (key === SIGNATURE_ROOT) return groups.value.formats;
    if (key.startsWith(`${SIGNATURE_ROOT}:`)) {
      return (groups.value.byFormat.get(key.slice(SIGNATURE_ROOT.length + 1)) ?? []).map(artifact => artifactEntry(artifact, options.formatLabel));
    }
    return fsTree.value.children.get(key) ?? [];
  }
  function parentOf(key: string): string {
    return lookup(key)?.parentKey ?? RECOVERY_ROOT;
  }

  // Descendant files per folder; rebuilt whenever either result set changes.
  const descendantCache = computed(() => {
    void fsTree.value; void groups.value;
    return new Map<string, RecoveryEntry[]>();
  });
  /** Every file below `key` (the file itself for a file), bounded against malformed cycles. */
  function descendantFiles(key: string): RecoveryEntry[] {
    const cache = descendantCache.value;
    const cached = cache.get(key);
    if (cached) return cached;
    const files: RecoveryEntry[] = [];
    const self = lookup(key);
    if (self?.kind === 'file') files.push(self);
    else {
      const pending = [key]; const seen = new Set<string>();
      while (pending.length) {
        const current = pending.pop()!;
        if (seen.has(current)) continue;
        seen.add(current);
        for (const child of childrenOf(current)) {
          if (child.kind === 'file') files.push(child); else pending.push(child.key);
        }
      }
    }
    cache.set(key, files);
    return files;
  }
  /** Selection keys a row stands for: itself, or every recoverable file below a folder. */
  function recoverableKeys(item: RecoveryEntry): string[] {
    if (item.kind === 'file') return isRecoverable(item) ? [item.key] : [];
    return descendantFiles(item.key).filter(isRecoverable).map(file => file.key);
  }

  const current = computed(() => lookup(currentKey.value));
  const breadcrumbs = computed(() => {
    const path: RecoveryEntry[] = [];
    let item = current.value;
    const seen = new Set<string>();
    while (item && !seen.has(item.key)) { seen.add(item.key); path.unshift(item); item = lookup(item.parentKey); }
    return path;
  });
  const rows = computed(() => {
    if (!hasFilters.value) return childrenOf(currentKey.value);
    const needle = query.value.trim().toLocaleLowerCase();
    return descendantFiles(currentKey.value).filter(item =>
      (categoryFilter.value === 'all' || entryCategory(item) === categoryFilter.value) && (!needle || entryMatches(item, needle)));
  });
  const selectedEntry = computed(() => lookup(selectedKey.value));

  /** Folder rows of the directory rail: roots, then expanded folders. */
  const treeRows = computed(() => {
    const result: RecoveryTreeRow[] = [];
    const visited = new Set<string>();
    function add(item: RecoveryEntry, depth: number) {
      if (visited.has(item.key) || depth > 64) return;
      visited.add(item.key);
      const folders = childrenOf(item.key).filter(child => child.kind !== 'file');
      result.push({ entry: item, depth, expandable: folders.length > 0 });
      if (expanded.value.has(item.key)) for (const child of folders) add(child, depth + 1);
    }
    for (const root of childrenOf(RECOVERY_ROOT)) add(root, 0);
    return result;
  });

  function navigate(key: string) {
    if (key !== RECOVERY_ROOT && (!lookup(key) || lookup(key)!.kind === 'file')) return;
    currentKey.value = key;
    selectedKey.value = null;
    const next = new Set(expanded.value);
    for (let item = lookup(key); item; item = lookup(item.parentKey)) next.add(item.key);
    expanded.value = next;
  }
  function goUp() {
    if (currentKey.value !== RECOVERY_ROOT) navigate(parentOf(currentKey.value));
  }
  function toggleExpanded(key: string) {
    const next = new Set(expanded.value);
    if (next.has(key)) next.delete(key); else next.add(key);
    expanded.value = next;
  }

  function isChecked(item: RecoveryEntry): boolean {
    const keys = recoverableKeys(item);
    return keys.length > 0 && keys.every(key => checkedKeys.value.has(key));
  }
  function isPartiallyChecked(item: RecoveryEntry): boolean {
    if (item.kind === 'file' || isChecked(item)) return false;
    return recoverableKeys(item).some(key => checkedKeys.value.has(key));
  }
  function setChecked(keys: string[], checked: boolean) {
    const next = new Set(checkedKeys.value);
    for (const key of keys) { if (checked) next.add(key); else next.delete(key); }
    checkedKeys.value = next;
  }
  function toggleChecked(item: RecoveryEntry) {
    setChecked(recoverableKeys(item), !isChecked(item));
  }
  const visibleKeys = computed(() => [...new Set(rows.value.flatMap(recoverableKeys))]);
  const allVisibleChecked = computed(() => visibleKeys.value.length > 0 && visibleKeys.value.every(key => checkedKeys.value.has(key)));
  const someVisibleChecked = computed(() => !allVisibleChecked.value && visibleKeys.value.some(key => checkedKeys.value.has(key)));
  function toggleAllVisible() { setChecked(visibleKeys.value, !allVisibleChecked.value); }
  function clearChecked() { checkedKeys.value = new Set(); }

  const checkedFileIds = computed(() => [...checkedKeys.value].filter(key => key.startsWith('f:')).map(key => key.slice(2)));
  const checkedArtifactIds = computed(() => [...checkedKeys.value].filter(key => key.startsWith('a:')).map(key => key.slice(2)));
  const checkedBytes = computed(() => {
    let total = 0;
    for (const key of checkedKeys.value) total += key.startsWith('a:') ? groups.value.byId.get(key.slice(2))?.length ?? 0 : fsTree.value.entries.get(key)?.bytes ?? 0;
    return total;
  });
  /** Every recoverable filesystem record of the scan, for "recover all". */
  const recoverableFileIds = computed(() => [...fsTree.value.entries.values()].filter(isRecoverable).map(item => item.candidate!.id));

  function applyFilters() {
    query.value = queryDraft.value.trim();
    categoryFilter.value = categoryFilterDraft.value;
  }
  function clearFilters() {
    queryDraft.value = query.value = '';
    categoryFilterDraft.value = categoryFilter.value = 'all';
  }
  function reset() {
    currentKey.value = RECOVERY_ROOT;
    selectedKey.value = null;
    checkedKeys.value = new Set();
    expanded.value = new Set();
    clearFilters();
  }

  watch(options.context, reset, { flush: 'sync' });
  // A folder that disappeared with its results falls back to the nearest root.
  watch([fsTree, groups], () => {
    if (currentKey.value !== RECOVERY_ROOT && !lookup(currentKey.value)) currentKey.value = RECOVERY_ROOT;
    if (selectedKey.value && !lookup(selectedKey.value)) selectedKey.value = null;
  });

  return {
    currentKey, current, breadcrumbs, rows, treeRows, expanded, selectedKey, selectedEntry, checkedKeys,
    query, queryDraft, categoryFilter, categoryFilterDraft, hasFilters,
    allVisibleChecked, someVisibleChecked, checkedFileIds, checkedArtifactIds, checkedBytes, recoverableFileIds,
    lookup, childrenOf, navigate, goUp, toggleExpanded, isChecked, isPartiallyChecked, toggleChecked, toggleAllVisible,
    clearChecked, applyFilters, clearFilters, reset,
  };
}
