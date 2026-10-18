<template>
    <!-- 新建搜索任务对话框：关键词、正则、编码页、范围与选项。 -->
    <Teleport to="body">
      <Transition name="ui-modal">
      <div v-if="open" class="ui-modal-overlay gs-modal-overlay" @keydown.esc.stop="emit('close')">
        <div class="ui-modal gs-modal" role="dialog" aria-modal="true" :aria-label="tr('新建搜索任务')" :aria-labelledby="titleId" tabindex="-1">
          <header class="ui-modal-header">
            <span class="ui-modal-icon"><Search :size="18" /></span>
            <div class="ui-modal-heading">
              <h2 :id="titleId" class="ui-modal-title">{{ tr("新建搜索") }}</h2>
              <p class="ui-modal-subtitle">{{ tr("在检材的文件内容中查找关键词或正则表达式") }}</p>
            </div>
            <button type="button" class="ui-icon-btn" :title="tr('关闭')" :aria-label="tr('关闭')" :disabled="starting" @click="emit('close')">
              <X :size="16" />
            </button>
          </header>
          <div class="ui-modal-body gs-modal-body">
            <label class="ui-field">
              <span class="ui-field-label">{{ tr("关键词（每行一个）") }}</span>
              <textarea
                class="ui-textarea keyword-textarea"
                v-model="draft.keysText"
                rows="4"
                spellcheck="false"
                :placeholder="tr('例如：password\n每行一个关键词，可同时搜索多个')"
                :ref="elements.keyword"
                :aria-invalid="patternError ? 'true' : undefined"
                :aria-describedby="patternsNoteId"
                @blur="patternsTouched = true"
              ></textarea>
              <p :id="patternsNoteId" :class="patternError ? 'ui-field-error' : 'ui-field-hint'" :role="patternError ? 'alert' : undefined">{{ patternError || tr("至少输入一个关键词，或在高级选项中填写正则表达式") }}</p>
            </label>
            <details v-disclosure-motion class="search-disclosure">
              <summary><span>{{ tr("搜索范围") }}</span><small>{{ scopeSummary }}</small></summary>
              <div class="disclosure-body">
                <div class="ui-field">
                  <span class="ui-field-label">{{ tr("搜索范围") }}</span>
                  <div class="ui-seg gs-scope-tabs" role="tablist">
                    <button
                      v-for="tab in SCOPE_TABS"
                      :key="tab.id"
                      type="button"
                      class="ui-seg-item"
                      :class="{ active: draft.scopeMode === tab.id }"
                      role="tab"
                      :aria-selected="draft.scopeMode === tab.id"
                      @click="draft.scopeMode = tab.id"
                    >
                      {{ tr(tab.label) }}
                    </button>
                  </div>
                  <!-- 按分区 -->
                  <div v-if="draft.scopeMode === 'partitions'" class="gs-scope-body">
                    <div v-if="loadingPartitions" class="task-state">
                      <Loader2 :size="13" class="spin" /><span>{{ tr("正在读取分区…") }}</span>
                    </div>
                    <div v-else-if="partitionOptions.length === 0" class="gs-scope-empty">
                      {{ tr("未检测到分区（整个镜像按单一文件系统处理，保持默认范围即可）") }}
                    </div>
                    <label v-else class="ui-check">
                      <input
                        type="checkbox"
                        :checked="draft.partitions.length === 0"
                        @change="draft.partitions = []"
                      />
                      <span>{{ tr("全部分区（默认）") }}</span>
                    </label>
                    <div v-if="partitionOptions.length > 0" class="gs-partition-grid">
                      <label
                        v-for="option in partitionOptions"
                        :key="option.id"
                        class="ui-check"
                        :title="option.title"
                      >
                        <input
                          type="checkbox"
                          :value="option.id"
                          v-model="draft.partitions"
                        />
                        <span>{{ option.label }}</span>
                      </label>
                    </div>
                  </div>
                  <!-- 按标签 -->
                  <div v-else-if="draft.scopeMode === 'tags'" class="gs-scope-body">
                    <label
                      v-for="tag in FILE_TAGS"
                      :key="tag"
                      class="ui-check"
                    >
                      <input type="checkbox" :value="tag" v-model="draft.tags" />
                      <span class="tag-chip" :class="TAG_STYLE[tag]">{{ tr(tag) }}</span>
                    </label>
                  </div>
                  <!-- 按目录 -->
                  <div v-else class="gs-scope-body">
                    <textarea
                      class="ui-textarea mono"
                      v-model="draft.treeNodesText"
                      rows="3"
                      spellcheck="false"
                      :placeholder="tr('每行一个卷内目录或文件路径，例如：\n/Users/alice/Documents\n/Windows/System32/config')"
                    ></textarea>
                    <span class="gs-scope-hint">{{ tr("目录会连同子目录一起搜索；留空表示不限制") }}</span>
                  </div>
                </div>

              </div>
            </details>
            <details v-disclosure-motion class="search-disclosure" :open="hasAdvancedErrors">
              <summary><span>{{ tr("高级选项") }}</span><small>{{ advancedSummary }}</small></summary>
              <div class="disclosure-body">
                <label class="ui-field">
                  <span class="ui-field-label">{{ tr("正则表达式（每行一个）") }}</span>
                  <textarea
                    class="ui-textarea mono"
                    v-model="draft.regexesText"
                    rows="3"
                    spellcheck="false"
                    placeholder="\d{17}[0-9X]&#10;pass\w+"
                    :aria-invalid="patternError ? 'true' : undefined"
                    :aria-describedby="patternsNoteId"
                    @blur="patternsTouched = true"
                  ></textarea>
                </label>
                <div class="ui-field">
                  <span class="ui-field-label">{{ tr("搜索编码") }}</span>
                  <div class="gs-chip-row">
                    <button
                      v-for="page in CODE_PAGES"
                      :key="page"
                      type="button"
                      class="ui-btn sm mono"
                      :aria-pressed="draft.codepages.includes(page)"
                      :aria-describedby="errors.codepages ? codepagesNoteId : undefined"
                      @click="emit('toggle-code-page', page)"
                    >
                      {{ page }}
                    </button>
                  </div>
                  <p v-if="errors.codepages" :id="codepagesNoteId" class="ui-field-error" role="alert">{{ errors.codepages }}</p>
                </div>

                <div class="gs-options-row">
                  <label class="ui-check" :title="tr('关闭后只搜索未删除的文件')">
                    <input type="checkbox" v-model="draft.searchDeleted" />
                    <span>{{ tr("搜索已删除文件") }}</span>
                  </label>
                  <label class="ui-check" :title="tr('同时搜索各分区的未分配空间和分区间隙')">
                    <input type="checkbox" v-model="draft.searchEmptySpace" />
                    <span>{{ tr("搜索未分配空间") }}</span>
                  </label>
                  <label class="ui-check" :title="tr('为命中文件计算 MD5 和 SHA256，供核验及去重使用')">
                    <input type="checkbox" v-model="draft.calcHash" />
                    <span>{{ tr("命中文件计算哈希") }}</span>
                  </label>
                  <label class="ui-check" :title="tr('区分大小写（默认不区分）')">
                    <input type="checkbox" v-model="draft.matchCase" />
                    <span>{{ tr("匹配大小写") }}</span>
                  </label>
                  <label class="ui-check" :title="tr('每个文件找到首个命中后即转到下一个文件')">
                    <input type="checkbox" v-model="draft.stopAfterFirst" />
                    <span>{{ tr("每个文件只保留首个命中") }}</span>
                  </label>
                </div>

                <div class="gs-caps-row">
                  <label class="gs-cap">
                    <span>{{ tr("每文件命中上限") }}</span>
                    <input v-model.number="draft.perFileCap" class="ui-input sm mono" type="number" min="1" step="1" :aria-invalid="errors.perFileCap ? 'true' : undefined" :aria-describedby="perFileNoteId" />
                    <span :id="perFileNoteId" :class="errors.perFileCap ? 'ui-field-error' : 'ui-field-hint'" :role="errors.perFileCap ? 'alert' : undefined">{{ errors.perFileCap || tr("填写大于 0 的整数") }}</span>
                  </label>
                  <label class="gs-cap">
                    <span>{{ tr("全局命中上限") }}</span>
                    <input v-model.number="draft.globalCap" class="ui-input sm mono" type="number" min="1" step="1" :aria-invalid="errors.globalCap ? 'true' : undefined" :aria-describedby="globalNoteId" />
                    <span :id="globalNoteId" :class="errors.globalCap ? 'ui-field-error' : 'ui-field-hint'" :role="errors.globalCap ? 'alert' : undefined">{{ errors.globalCap || tr("填写大于 0 的整数") }}</span>
                  </label>
                </div>

              </div>
            </details>
          </div>
          <footer class="ui-modal-footer gs-modal-footer">
            <button class="ui-btn" type="button" :disabled="starting" @click="emit('close')">{{ tr("取消") }}</button>
            <button
              class="ui-btn primary"
              type="button"
              :disabled="starting || Boolean(dialogError)"
              @click="emit('submit')"
            >
              <Loader2 v-if="starting" :size="13" class="spin" />
              <Search v-else :size="13" />
              <span>{{ tr("开始搜索") }}</span>
            </button>
          </footer>
        </div>
      </div>
      </Transition>
    </Teleport>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { Loader2, Search, X } from 'lucide-vue-next';
import { computed, ref, useId, watch, type Ref } from 'vue';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import type { SearchCodePage } from '../../../lib/ipc/search';
import { vDisclosureMotion } from '../../../composables/uiMotionDirectives';
import { FILE_TAGS, TAG_STYLE } from '../../files/model/tags';
import { CODE_PAGES, SCOPE_TABS, searchConfigurationErrors, type SearchDraft } from '../model/searchConfiguration';
const props = defineProps<{ open: boolean; draft: SearchDraft; elements: { keyword: Ref<HTMLTextAreaElement | null> }; starting: boolean; loadingPartitions: boolean; partitionOptions: { id: string; label: string; title: string }[]; scopeSummary: string; advancedSummary: string; dialogError: string | null }>();
const titleId = useId();
const patternsNoteId = useId();
const codepagesNoteId = useId();
const perFileNoteId = useId();
const globalNoteId = useId();
const patternsTouched = ref(false);
const errors = computed(() => searchConfigurationErrors(props.draft, tr));
const patternError = computed(() => patternsTouched.value ? errors.value.patterns : null);
const hasAdvancedErrors = computed(() => Boolean(errors.value.codepages || errors.value.perFileCap || errors.value.globalCap));
watch(() => props.open, () => { patternsTouched.value = false; });
useDialogFocus(() => props.open, props.elements.keyword);
const emit = defineEmits<{ close: []; submit: []; 'toggle-code-page': [page: SearchCodePage] }>();
</script>
<style scoped src="../../../styles/analysis-workspace.css"></style>
