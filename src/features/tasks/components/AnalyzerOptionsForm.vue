<template>
  <div class="analyzer-options" :class="{ compact }">
    <div v-for="field in fields" :key="field.key" class="ui-field">
      <template v-if="field.option.kind === 'multi'">
        <span :id="fieldId(field)" class="ui-field-label">{{ fieldLabel(field) }}</span>
        <div class="option-choices" role="group" :aria-labelledby="fieldId(field)">
          <label v-for="choice in field.option.choices" :key="choice.id" class="ui-check">
            <input type="checkbox" :checked="selectedChoices(field).includes(choice.id)" :disabled="disabled" @change="toggleChoice(field, choice.id)" />{{ choiceLabel(field, choice) }}
          </label>
        </div>
      </template>
      <template v-else-if="field.option.kind === 'choice'">
        <span :id="fieldId(field)" class="ui-field-label">{{ fieldLabel(field) }}</span>
        <UiSelect :model-value="String(valueOf(field) ?? '')" :options="field.option.choices.map(choice => ({ value: choice.id, label: choiceLabel(field, choice) }))" :disabled="disabled" :aria-labelledby="fieldId(field)" @update:model-value="value => update(field, value)" />
      </template>
      <label v-else-if="field.option.kind === 'flag'" class="ui-check">
        <input type="checkbox" class="ui-switch" :checked="valueOf(field) === true" :disabled="disabled" @change="update(field, ($event.target as HTMLInputElement).checked)" />{{ fieldLabel(field) }}
      </label>
      <template v-else>
        <label :for="fieldId(field)" class="ui-field-label">{{ fieldLabel(field) }}</label>
        <input :id="fieldId(field)" class="ui-input" type="text" maxlength="4096" :value="String(valueOf(field) ?? '')" :disabled="disabled" @input="update(field, ($event.target as HTMLInputElement).value)" />
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

/** The options analyzers declare, edited before a run (see `model/analyzerOptions`).
 *  Values the examiner leaves alone keep the analyzer's defaults. */
import UiSelect from '../../../components/ui/UiSelect.vue';
import type { SchedFunctionChoice } from '../../../lib/ipc/tasks';
import { pluginTextKey } from '../../../lib/models/pluginText';
import type { AnalyzerOptionField } from '../model/analyzerOptions';
import { usePluginCatalog } from '../composables/usePluginCatalog';

defineProps<{ fields: readonly AnalyzerOptionField[]; disabled?: boolean; compact?: boolean }>();
const model = defineModel<Record<string, unknown>>({ required: true });
const catalog = usePluginCatalog();
const uid = Math.random().toString(36).slice(2, 8);

function fieldId(field: AnalyzerOptionField): string {
  return `analyzer-option-${uid}-${field.key}`;
}

function fieldLabel(field: AnalyzerOptionField): string {
  const label = catalog.text(field.pluginId, pluginTextKey.option(field.analyzer, field.option.id), field.option.label);
  return field.functionName ? tr("{0}：{1}", { 0: field.functionName, 1: label }) : label;
}

function choiceLabel(field: AnalyzerOptionField, choice: SchedFunctionChoice): string {
  return catalog.text(field.pluginId, pluginTextKey.choice(field.analyzer, field.option.id, choice.id), choice.label);
}

function valueOf(field: AnalyzerOptionField): unknown {
  return Object.hasOwn(model.value, field.key) ? model.value[field.key] : field.option.default;
}

function selectedChoices(field: AnalyzerOptionField): string[] {
  const value = valueOf(field);
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
}

function update(field: AnalyzerOptionField, value: unknown): void {
  model.value = { ...model.value, [field.key]: value };
}

function toggleChoice(field: AnalyzerOptionField, choice: string): void {
  const current = selectedChoices(field);
  update(field, current.includes(choice) ? current.filter(item => item !== choice) : [...current, choice]);
}
</script>

<style scoped>
.analyzer-options { display: flex; flex-direction: column; gap: 16px; }
.option-choices { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 8px 12px; }
.analyzer-options.compact { gap: 8px; }
.analyzer-options.compact .ui-field { gap: 4px; }
.analyzer-options.compact .option-choices { display: flex; flex-wrap: wrap; gap: 4px 12px; }
</style>
