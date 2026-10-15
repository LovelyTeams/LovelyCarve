
import { tr } from '../../../composables/useLocale';
import { computed, nextTick, onDeactivated, onScopeDispose, ref, watch, type Ref } from 'vue';
import { carveFormats, type CarveCategory, type CarveFormatInfo, type CarveStartOptions, type CarveScope } from '../../../lib/ipc/carve';
import type { DiskPartition, ImageInfo } from '../../../lib/ipc/image';
import { formatBytes } from '../../../utils/format';
import { categories } from '../model/carveView';
export function useCarveConfiguration(options: { imageId: () => string; imageInfo: () => ImageInfo | null; starting: () => boolean }) {
  const GIB = 1024 ** 3;
  let disposed = false;
  let focusGeneration = 0;
  const configExpanded = ref(false);
  const configurationDialog = ref<HTMLElement | null>(null);
  let dialogOpener: HTMLElement | null = null;
  const formats = ref<CarveFormatInfo[]>([]);
  const formatGroups = computed(() => categories.map(category => ({
    ...category, formats: formats.value.filter(format => format.category === category.id),
  })).filter(group => group.formats.length > 0));
  function toggleGroup(category: CarveCategory) {
    const members = formats.value.filter(format => format.category === category);
    const selected = members.every(format => selectedFormatIds.value.has(format.id));
    replaceSet(selectedFormatIds, next => members.forEach(format => {
      if (selected) next.delete(format.id); else next.add(format.id);
    }));
  }
  const selectedFormatIds = ref<Set<string>>(new Set());
  const scope = ref<CarveScope>('whole');
  const partitionId = ref<number | null>(null);
  const maxFileSize = ref(10 * GIB);
  const maxResults = ref(20_000);
  function replaceSet(target: Ref<Set<string>>, update: (next: Set<string>) => void) {
    const next = new Set(target.value);
    update(next);
    target.value = next;
  }

  function toggleFormat(id: string) {
    replaceSet(selectedFormatIds, next => {
      if (next.has(id)) next.delete(id);
      else next.add(id);
    });
  }

  function selectAllFormats() {
    selectedFormatIds.value = new Set(formats.value.map(format => format.id));
  }

  function clearFormats() {
    selectedFormatIds.value = new Set();
  }

  const partitions = computed(() => options.imageInfo()?.partitions ?? []);
  const partitionOptions = computed(() => partitions.value.map(partition => ({ value: partition.id, label: partitionLabel(partition) })));
  function partitionLabel(partition: DiskPartition): string {
    const name = partition.label?.trim() || tr("分区 {0}", { 0: partition.id });
    return `${name} · ${partition.kind.toUpperCase()} · ${formatBytes(partition.length_lba * (options.imageInfo()?.sector_size ?? 512))}`;
  }

  watch(partitions, value => {
    if (partitionId.value == null || !value.some(partition => partition.id === partitionId.value)) partitionId.value = value[0]?.id ?? null;
  }, { immediate: true });
  function buildOptions(): CarveStartOptions {
    return {
      format_ids: [...selectedFormatIds.value], scope: scope.value, partition_id: scope.value === 'partition' ? partitionId.value : null,
      max_file_size: maxFileSize.value, max_results: maxResults.value
    };
  }
  async function loadFormats() { const loaded = await carveFormats(); if (!disposed) { formats.value = loaded; selectAllFormats(); } }

  watch(configExpanded, async visible => {
    const captured = ++focusGeneration;
    if (visible) {
      dialogOpener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      await nextTick();
      if (!disposed && captured === focusGeneration && configExpanded.value) configurationDialog.value?.querySelector<HTMLButtonElement>('button')?.focus();
    } else if (!disposed) dialogOpener?.focus();
  });
  function closeConfiguration() { if (!options.starting()) configExpanded.value = false; }
  watch(options.imageId, () => { focusGeneration++; configExpanded.value = false; }, { flush: 'sync' });
  onDeactivated(() => { focusGeneration++; configExpanded.value = false; });
  onScopeDispose(() => { disposed = true; focusGeneration++; });
  return {
    formats, formatGroups, selectedFormatIds, scope, partitionId, maxFileSize, maxResults, partitions, partitionOptions,
    configExpanded, elements: { dialog: configurationDialog }, toggleGroup, toggleFormat, selectAllFormats, clearFormats,
    buildOptions, loadFormats, closeConfiguration
  };
}
