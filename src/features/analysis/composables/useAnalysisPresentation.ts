import { computed, ref, type Ref } from 'vue';
import type { AnalysisResultNode, RecordColumn, RecordPropertyLayout } from '../../../lib/ipc/analysisResults';
import { usePluginCatalog } from '../../tasks/api';
import { compatiblePropertyLayout, resolvePropertyLayout, type AnalysisPresentation } from '../model/analysisPresentation';

const STORAGE_KEY = 'lovelycarve-analysis-presentation';

/** Per-plugin record-type preferences; defaults come from the saved schema, then a compatible installed schema. */
export function useAnalysisPresentation(node: Ref<AnalysisResultNode | null>, columns: Ref<RecordColumn[]>, saved: Ref<RecordPropertyLayout | null>) {
  const catalog = usePluginCatalog();
  const preferences = ref<Record<string, AnalysisPresentation>>({});
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      preferences.value = Object.fromEntries(Object.entries(parsed).filter(([, value]) => value === 'table' || value === 'properties'));
    }
  } catch { /* An unavailable or invalid preference must not prevent reading evidence. */ }
  const preferenceKey = computed(() => node.value ? `${node.value.pluginId}::${node.value.artifactType}` : '');
  const propertyLayout = computed(() => {
    if (resolvePropertyLayout(saved.value, columns.value)) return saved.value;
    if (!node.value || node.value.legacy) return null;
    const declared = catalog.packages.value.find(item => item.id === node.value?.pluginId)?.records.find(item => item.recordType === node.value?.artifactType);
    return declared ? compatiblePropertyLayout(columns.value, declared.fields, declared.properties) : null;
  });
  const presentation = computed<AnalysisPresentation>(() => preferences.value[preferenceKey.value] ?? (propertyLayout.value ? 'properties' : 'table'));
  function setPresentation(value: AnalysisPresentation) {
    if (!preferenceKey.value) return;
    preferences.value = { ...preferences.value, [preferenceKey.value]: value };
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences.value)); }
    catch { /* Keep the current choice for this panel when storage is unavailable. */ }
  }
  return { propertyLayout, presentation, setPresentation };
}
