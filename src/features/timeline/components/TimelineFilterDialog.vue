<template>
    <Teleport to="body">
      <UiTransition kind="fade"><div v-if="dialog.open" class="ui-modal-overlay tl-modal-overlay">
        <div ref="dialogElement" class="ui-modal tl-modal" role="dialog" aria-modal="true" :aria-label="tr('时间线筛选配置')" :aria-labelledby="titleId" tabindex="-1" @keydown.esc.stop="emit('close')">
          <header class="ui-modal-header">
            <span class="ui-modal-icon"><SlidersHorizontal :size="18" /></span>
            <div class="ui-modal-heading">
              <h3 :id="titleId" class="ui-modal-title">{{ tr("事件类型") }}</h3>
              <p class="ui-modal-subtitle">{{ tr("默认显示全部类型。勾选后仅显示所选来源与事件类型。") }}</p>
            </div>
            <button type="button" class="ui-icon-btn" :title="tr('关闭筛选配置')" :aria-label="tr('关闭筛选配置')" @click="emit('close')">
              <X :size="16" />
            </button>
          </header>

          <section class="ui-modal-body tl-modal-section">
            <h4 class="ui-section-title">{{ tr("应用与数据类型") }}</h4>
            <div v-if="typeGroups.length === 0" class="ui-empty tl-modal-empty"><p class="ui-empty-desc">{{ tr("暂无维度数据") }}</p></div>
            <div v-for="group in typeGroups" :key="group.from_app" class="tl-type-group">
              <label class="ui-check tl-app-check">
                <input
                  type="checkbox"
                  :checked="fullyChecked(group)"
                  @change="emit('toggle-group', group)"
                />
                <span>{{ group.from_app }}</span>
              </label>
              <label
                v-for="info in group.types"
                :key="info.data_type"
                class="ui-check tl-type-check"
              >
                <input
                  type="checkbox"
                  :checked="pairChecked(group.from_app, info.data_type)"
                  @change="emit('toggle-pair', group.from_app, info.data_type)"
                />
                {{ info.data_type_name }}
                <span class="ui-count">{{ info.count.toLocaleString() }}</span>
              </label>
            </div>
          </section>

          <footer class="ui-modal-footer">
            <button type="button" class="ui-btn" @click="emit('close')">{{ tr("取消") }}</button>
            <button type="button" class="ui-btn primary" @click="emit('apply')">{{ tr("应用") }}</button>
          </footer>
        </div>
      </div></UiTransition>
    </Teleport>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { SlidersHorizontal, X } from 'lucide-vue-next';
import { ref, useId } from 'vue';
import UiTransition from '../../../components/ui/UiTransition.vue';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import type { TimelineTypeGroup, TimelineTypePair } from '../../../lib/ipc/timeline';
const props = defineProps<{ dialog: { open: boolean; pairs: TimelineTypePair[] }; typeGroups: TimelineTypeGroup[] }>();
const dialogElement = ref<HTMLElement | null>(null);
const titleId = useId();
useDialogFocus(() => props.dialog.open, dialogElement);
const emit = defineEmits<{ close: []; apply: []; 'toggle-group': [group: TimelineTypeGroup]; 'toggle-pair': [fromApp: string, dataType: number] }>();
const pairChecked = (fromApp: string, dataType: number) => props.dialog.pairs.some(pair => pair.from_app === fromApp && pair.data_type === dataType);
const fullyChecked = (group: TimelineTypeGroup) => group.types.every(type => pairChecked(group.from_app, type.data_type));
</script>

<style scoped src="../styles/timeline-workspace.css"></style>
