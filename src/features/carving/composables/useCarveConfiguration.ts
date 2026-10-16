
import { tr } from '../../../composables/useLocale';
import { computed, nextTick, onDeactivated, onScopeDispose, ref, watch, type Ref } from 'vue';
import { carveFormats, type CarveCategory, type CarveFormatInfo, type CarveStartOptions, type CarveScope, type RecoveryFilesystems } from '../../../lib/ipc/carve';
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
  const startSector = ref('0');
  const sectorCount = ref('');
  const maxFileSize = ref(10 * GIB);
  const maxResults = ref(20_000);
  /** Filesystem recovery and signature carving are both on by default. */
  const filesystems = ref<RecoveryFilesystems>({ fat: true, ntfs: true, exfat: true, include_existing: false });
  const signatures = ref(true);
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
  const sectorSize = computed(() => options.imageInfo()?.sector_size ?? 512);
  const totalSectors = computed(() => Math.floor((options.imageInfo()?.size_bytes ?? 0) / sectorSize.value));
  /** Sector range input problem; empty count scans to the end of the evidence. */
  const rangeError = computed(() => {
    if (scope.value !== 'range') return '';
    const start = Number(startSector.value);
    const count = sectorCount.value.trim() ? Number(sectorCount.value) : totalSectors.value - start;
    const valid = /^\d+$/.test(startSector.value) && Number.isSafeInteger(start) && start < totalSectors.value
      && (!sectorCount.value.trim() || /^\d+$/.test(sectorCount.value.trim()))
      && Number.isSafeInteger(count) && count > 0 && start + count <= totalSectors.value;
    return valid ? '' : tr("请输入镜像范围内的整数扇区范围。");
  });
  const methodError = computed(() => {
    const filesystem = filesystems.value.fat || filesystems.value.ntfs || filesystems.value.exfat;
    if (!filesystem && !signatures.value) return tr("请至少选择一种恢复方式。");
    if (signatures.value && selectedFormatIds.value.size === 0) return tr("签名雕刻至少需要一种文件类型。");
    return '';
  });

  watch(partitions, value => {
    if (partitionId.value == null || !value.some(partition => partition.id === partitionId.value)) partitionId.value = value[0]?.id ?? null;
  }, { immediate: true });
  function buildOptions(): CarveStartOptions {
    const start = Number(startSector.value);
    return {
      format_ids: signatures.value ? [...selectedFormatIds.value] : [], scope: scope.value, partition_id: scope.value === 'partition' ? partitionId.value : null,
      start_sector: scope.value === 'range' ? start : null,
      sector_count: scope.value === 'range' ? (sectorCount.value.trim() ? Number(sectorCount.value) : totalSectors.value - start) : null,
      max_file_size: maxFileSize.value, max_results: maxResults.value,
      filesystems: { ...filesystems.value }, signatures: signatures.value,
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
  watch(options.imageId, () => {
    focusGeneration++; configExpanded.value = false;
    startSector.value = '0'; sectorCount.value = '';
    if (scope.value === 'range') scope.value = 'whole';
  }, { flush: 'sync' });
  onDeactivated(() => { focusGeneration++; configExpanded.value = false; });
  onScopeDispose(() => { disposed = true; focusGeneration++; });
  return {
    formats, formatGroups, selectedFormatIds, scope, partitionId, startSector, sectorCount, sectorSize, maxFileSize, maxResults,
    filesystems, signatures, partitions, partitionOptions, rangeError, methodError,
    configExpanded, elements: { dialog: configurationDialog }, toggleGroup, toggleFormat, selectAllFormats, clearFormats,
    buildOptions, loadFormats, closeConfiguration
  };
}
