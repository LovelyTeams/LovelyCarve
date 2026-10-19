<template>
  <div class="analyzer-options">
    <div v-for="option in fn.options" :key="option.id" class="ui-field">
      <template v-if="option.kind === 'multi'">
        <span :id="fieldId(option.id)" class="ui-field-label">{{ optionLabel(option) }}</span>
        <div class="option-choices" role="group" :aria-labelledby="fieldId(option.id)">
          <label v-for="choice in option.choices" :key="choice.id" class="ui-check">
            <input type="checkbox" :checked="selectedChoices(option).includes(choice.id)" :disabled="disabled" @change="toggleChoice(option, choice.id)" />{{ choiceLabel(option, choice) }}
          </label>
        </div>
        <p class="ui-field-hint">{{ tr("不勾选即运行全部子项。") }}</p>
      </template>
      <template v-else-if="option.kind === 'choice'">
        <span :id="fieldId(option.id)" class="ui-field-label">{{ optionLabel(option) }}</span>
        <UiSelect :model-value="String(valueOf(option) ?? '')" :options="option.choices.map(choice => ({ value: choice.id, label: choiceLabel(option, choice) }))" :disabled="disabled" :aria-labelledby="fieldId(option.id)" @update:model-value="value => update(option.id, value)" />
      </template>
      <label v-else-if="option.kind === 'flag'" class="ui-check">
        <input type="checkbox" class="ui-switch" :checked="valueOf(option) === true" :disabled="disabled" @change="update(option.id, ($event.target as HTMLInputElement).checked)" />{{ optionLabel(option) }}
      </label>
      <template v-else>
        <label :for="fieldId(option.id)" class="ui-field-label">{{ optionLabel(option) }}</label>
        <input :id="fieldId(option.id)" class="ui-input" type="text" maxlength="4096" :value="String(valueOf(option) ?? '')" :disabled="disabled" @input="update(option.id, ($event.target as HTMLInputElement).value)" />
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

/** The options an analyzer declares, edited before a run. Values the
 *  examiner leaves alone keep the analyzer's defaults. */
import UiSelect from '../../../components/ui/UiSelect.vue';
import type { SchedFunction, SchedFunctionChoice, SchedFunctionOption } from '../../../lib/ipc/tasks';
import { pluginTextKey } from '../../../lib/models/pluginText';
import { usePluginCatalog } from '../composables/usePluginCatalog';

const props = defineProps<{ fn: SchedFunction; disabled?: boolean }>();
const model = defineModel<Record<string, unknown>>({ required: true });
const catalog = usePluginCatalog();
const uid = Math.random().toString(36).slice(2, 8);

function fieldId(optionId: string): string {
  return `analyzer-option-${uid}-${optionId}`;
}

function optionLabel(option: SchedFunctionOption): string {
  return catalog.text(props.fn.pluginId, pluginTextKey.option(props.fn.analyzer, option.id), option.label);
}

function choiceLabel(option: SchedFunctionOption, choice: SchedFunctionChoice): string {
  return catalog.text(props.fn.pluginId, pluginTextKey.choice(props.fn.analyzer, option.id, choice.id), choice.label);
}

function valueOf(option: SchedFunctionOption): unknown {
  return Object.hasOwn(model.value, option.id) ? model.value[option.id] : option.default;
}

function selectedChoices(option: SchedFunctionOption): string[] {
  const value = valueOf(option);
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
}

function update(id: string, value: unknown): void {
  model.value = { ...model.value, [id]: value };
}

function toggleChoice(option: SchedFunctionOption, choice: string): void {
  const current = selectedChoices(option);
  update(option.id, current.includes(choice) ? current.filter(item => item !== choice) : [...current, choice]);
}
</script>

<style scoped>
.analyzer-options { display: flex; flex-direction: column; gap: 16px; }
.option-choices { display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 8px 12px; }
</style>
