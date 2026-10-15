<template>
<Teleport to="body">
  <UiTransition kind="pop"><div
    v-if="filterPopover.visible"
    :ref="controls.filterPopoverEl"
    class="ui-menu disk-filter-popover"
    :style="{ left: `${filterPopover.x}px`, top: `${filterPopover.y}px`, width: `${filterPopover.width}px` }"
  >
    <div class="disk-filter-head">
      <span class="disk-filter-title">{{ tr("过滤 ·") }} {{ tr(filterPopoverColumn?.label ?? '') }}</span>
      <button class="ui-icon-btn sm" :title="tr('关闭')" :aria-label="tr('关闭过滤')" @click="hideFilterPopover">
        <X :size="14" />
      </button>
    </div>

    <template v-if="filterPopoverColumn?.filterMode === 'size'">
      <div
        class="disk-size-form"
        @keydown.enter="onSizeFormEnter"
        @keydown.esc="onFilterCloseKey"
      >
        <div class="disk-size-row">
          <input type="checkbox" v-model="sizeFilter.leEnabled" :title="tr('启用大小上限')" />
          <span class="disk-size-op">≤</span>
          <input
            class="ui-input sm mono disk-size-num"
            type="text"
            inputmode="decimal"
            placeholder="1.2"
            :aria-label="tr('文件大小上限')"
            :aria-invalid="sizeFilterErrors.le ? 'true' : undefined"
            :aria-describedby="sizeFilterErrors.le ? `${fieldId}-size-upper` : undefined"
            v-model="sizeFilter.leValue"
            @input="sizeFilter.leEnabled = true"
          />
          <UiSelect v-model="sizeFilter.leUnit" class="sm mono disk-size-unit" :options="SIZE_UNIT_OPTIONS" :aria-label="tr('大小上限单位')" />
        </div>
        <p v-if="sizeFilterErrors.le" :id="`${fieldId}-size-upper`" class="ui-field-error" role="alert">{{ sizeFilterErrors.le }}</p>
        <div class="disk-size-row">
          <input type="checkbox" v-model="sizeFilter.geEnabled" :title="tr('启用大小下限')" />
          <span class="disk-size-op">≥</span>
          <input
            class="ui-input sm mono disk-size-num"
            type="text"
            inputmode="decimal"
            placeholder="3.4"
            :aria-label="tr('文件大小下限')"
            :aria-invalid="sizeFilterErrors.ge ? 'true' : undefined"
            :aria-describedby="sizeFilterErrors.ge ? `${fieldId}-size-lower` : undefined"
            v-model="sizeFilter.geValue"
            @input="sizeFilter.geEnabled = true"
          />
          <UiSelect v-model="sizeFilter.geUnit" class="sm mono disk-size-unit" :options="SIZE_UNIT_OPTIONS" :aria-label="tr('大小下限单位')" />
        </div>
        <p v-if="sizeFilterErrors.ge" :id="`${fieldId}-size-lower`" class="ui-field-error" role="alert">{{ sizeFilterErrors.ge }}</p>
        <div class="disk-size-row">
          <input type="checkbox" v-model="sizeFilter.modEnabled" :title="tr('按字节数整除余数筛选（如 mod 512 = 0 找扇区对齐的文件）')" />
          <span class="disk-size-op">mod</span>
          <input
            class="ui-input sm mono disk-size-num"
            type="text"
            inputmode="numeric"
            placeholder="512"
            :aria-label="tr('大小整除的除数')"
            :aria-invalid="sizeFilterErrors.mod ? 'true' : undefined"
            :aria-describedby="sizeFilterErrors.mod ? `${fieldId}-size-divisor` : undefined"
            v-model="sizeFilter.modValue"
            @input="sizeFilter.modEnabled = true"
          />
          <span class="disk-size-op">=</span>
          <input
            class="ui-input sm mono disk-size-num"
            type="text"
            inputmode="numeric"
            placeholder="0"
            :aria-label="tr('大小整除的余数')"
            :aria-invalid="sizeFilterErrors.remainder ? 'true' : undefined"
            :aria-describedby="sizeFilterErrors.remainder ? `${fieldId}-size-remainder` : undefined"
            v-model="sizeFilter.modRemainder"
            @input="sizeFilter.modEnabled = true"
          />
        </div>
        <p v-if="sizeFilterErrors.mod" :id="`${fieldId}-size-divisor`" class="ui-field-error" role="alert">{{ sizeFilterErrors.mod }}</p>
        <p v-if="sizeFilterErrors.remainder" :id="`${fieldId}-size-remainder`" class="ui-field-error" role="alert">{{ sizeFilterErrors.remainder }}</p>
        <div class="disk-filter-actions">
          <button class="ui-btn sm primary" @click="applySizeFilter">
            <Filter :size="12" />
            <span>{{ tr("激活") }}</span>
          </button>
          <button class="ui-btn sm" :disabled="!sizeFilter.active" @click="disableSizeFilter">
            {{ tr("禁用") }}
          </button>
        </div>
      </div>
    </template>

    <template v-else-if="popoverTimeFilter">
      <div
        class="disk-size-form"
        @keydown.enter="onTimeFormEnter"
        @keydown.esc="onFilterCloseKey"
      >
        <div class="disk-size-row">
          <input type="checkbox" v-model="popoverTimeFilter.fromEnabled" :title="tr('启用起始时间')" />
          <span class="disk-size-op">{{ tr("自") }}</span>
          <div class="disk-time-wrap">
            <input
              class="ui-input sm mono disk-time-input"
              type="text"
              placeholder="yyyy-mm-dd hh:mm:ss"
              :title="tr('可只填日期（按整日）或省略秒（按整分钟）')"
              v-model="popoverTimeFilter.fromValue"
              :aria-label="tr('起始时间')"
              :aria-invalid="timeFilterErrors.from ? 'true' : undefined"
              :aria-describedby="timeFilterErrors.from ? `${fieldId}-time-from` : undefined"
              @input="popoverTimeFilter.fromEnabled = true"
            />
            <button
              class="ui-icon-btn sm disk-time-pick"
              :class="{ active: timeCalOpen === 'from' }"
              :title="tr('从日历选择')"
              :aria-label="tr('从日历选择起始时间')"
              @click="toggleTimeCal('from')"
            >
              <CalendarDays :size="12" />
            </button>
          </div>
        </div>
        <p v-if="timeFilterErrors.from" :id="`${fieldId}-time-from`" class="ui-field-error" role="alert">{{ timeFilterErrors.from }}</p>
        <div class="disk-size-row">
          <input type="checkbox" v-model="popoverTimeFilter.toEnabled" :title="tr('启用截止时间')" />
          <span class="disk-size-op">{{ tr("至") }}</span>
          <div class="disk-time-wrap">
            <input
              class="ui-input sm mono disk-time-input"
              type="text"
              placeholder="yyyy-mm-dd hh:mm:ss"
              :title="tr('可只填日期（按整日）或省略秒（按整分钟）')"
              v-model="popoverTimeFilter.toValue"
              :aria-label="tr('截止时间')"
              :aria-invalid="timeFilterErrors.to ? 'true' : undefined"
              :aria-describedby="timeFilterErrors.to ? `${fieldId}-time-to` : undefined"
              @input="popoverTimeFilter.toEnabled = true"
            />
            <button
              class="ui-icon-btn sm disk-time-pick"
              :class="{ active: timeCalOpen === 'to' }"
              :title="tr('从日历选择')"
              :aria-label="tr('从日历选择截止时间')"
              @click="toggleTimeCal('to')"
            >
              <CalendarDays :size="12" />
            </button>
          </div>
        </div>
        <p v-if="timeFilterErrors.to" :id="`${fieldId}-time-to`" class="ui-field-error" role="alert">{{ timeFilterErrors.to }}</p>
        <UiTransition kind="collapse"><div v-if="timeCalOpen" class="disk-time-cal">
          <div class="cal-main">
            <div class="disk-time-cal-head">
              <button class="cal-nav" :title="tr('上一年')" @click="moveTimeCal(-12)">
                <ChevronsLeft :size="13" />
              </button>
              <button class="cal-nav" :title="tr('上一月')" @click="moveTimeCal(-1)">
                <ChevronLeft :size="13" />
              </button>
              <span class="cal-title">
                {{ timeCalCursor.year }} {{ tr("年") }} {{ String(timeCalCursor.month + 1).padStart(2, '0') }} {{ tr("月") }}
              </span>
              <button class="cal-nav" :title="tr('下一月')" @click="moveTimeCal(1)">
                <ChevronRight :size="13" />
              </button>
              <button class="cal-nav" :title="tr('下一年')" @click="moveTimeCal(12)">
                <ChevronsRight :size="13" />
              </button>
            </div>
            <div class="disk-time-cal-grid">
              <span v-for="w in [tr('一'), tr('二'), tr('三'), tr('四'), tr('五'), tr('六'), tr('日')]" :key="w" class="cal-weekday">
                {{ w }}
              </span>
              <button
                v-for="c in timeCalCells"
                :key="c.iso"
                class="cal-day"
                :class="{ out: !c.inMonth, today: c.isToday, selected: c.isSelected }"
                :title="c.iso"
                @click="pickTimeCalDay(c.iso)"
              >{{ c.day }}</button>
            </div>
            <div class="disk-time-cal-foot">
              <button class="ui-btn link sm" @click="clearTimeCalRow">{{ tr("清除") }}</button>
              <button class="ui-btn link sm" @click="pickTimeCalToday">{{ tr("今天") }}</button>
            </div>
          </div>
          <div class="cal-time-cols">
            <div class="cal-time-col" :title="tr('时')">
              <button
                v-for="v in CAL_HOURS"
                :key="`h${v}`"
                class="cal-time-item"
                :class="{ selected: timeCalTime.h === v }"
                @click="pickCalTime('h', v)"
              >{{ v }}</button>
            </div>
            <div class="cal-time-col" :title="tr('分')">
              <button
                v-for="v in CAL_MINSEC"
                :key="`m${v}`"
                class="cal-time-item"
                :class="{ selected: timeCalTime.m === v }"
                @click="pickCalTime('m', v)"
              >{{ v }}</button>
            </div>
            <div class="cal-time-col" :title="tr('秒')">
              <button
                v-for="v in CAL_MINSEC"
                :key="`s${v}`"
                class="cal-time-item"
                :class="{ selected: timeCalTime.s === v }"
                @click="pickCalTime('s', v)"
              >{{ v }}</button>
            </div>
          </div>
        </div></UiTransition>
        <div class="disk-filter-actions">
          <button class="ui-btn sm primary" @click="applyPopoverTimeFilter">
            <Filter :size="12" />
            <span>{{ tr("激活") }}</span>
          </button>
          <button
            class="ui-btn sm"
            :disabled="!popoverTimeFilter.active"
            @click="disablePopoverTimeFilter"
          >
            {{ tr("禁用") }}
          </button>
        </div>
      </div>
    </template>

    <template v-else-if="filterPopoverColumn?.filterMode === 'bool'">
      <div class="disk-bool-options">
        <label v-for="opt in BOOL_FILTER_OPTIONS" :key="opt.value" class="ui-check disk-bool-option">
          <input
            type="radio"
            name="disk-bool-filter"
            :value="opt.value"
            v-model="filters[filterPopover.key]"
          />
          <span>{{ tr(opt.label) }}</span>
        </label>
      </div>
    </template>

    <template v-else>
      <div class="disk-filter-input-row">
        <Filter :size="12" class="disk-filter-funnel" />
        <input
          class="ui-input sm mono disk-filter-input"
          type="text"
          :placeholder="filterPopover.key === 'path' ? tr('输入 / 逐级补全，Tab 补全 ↑↓ 选择') : filterPopover.key === 'name' ? tr('关键字，支持 * ? 通配符') : tr('输入关键字')"
          v-model="filters[filterPopover.key]"
          @input="onTextFilterInput"
          @focus="onTextFilterFocus"
          @keydown="onTextFilterKeydown"
        />
      </div>
      <div v-if="pathSuggestVisible" class="disk-path-suggest">
        <div :ref="controls.pathSuggestEl" class="disk-path-suggest-list">
          <button
            v-for="(s, i) in pathSuggestMatches.list"
            :key="s.value"
            class="disk-path-suggest-item"
            :class="{ active: i === pathSuggestIndex }"
            :title="s.value"
            @pointermove="pathSuggestIndex = i"
            @click="onPathSuggestClick(s)"
          >
            <component :is="s.isDir ? Folder : FileIcon" :size="11" class="disk-path-suggest-ico" />
            <span class="disk-path-suggest-text">{{ s.pre }}<b v-if="s.hit" class="disk-path-suggest-hit">{{ s.hit }}</b>{{ s.post }}<span v-if="s.isDir" class="disk-path-suggest-slash">/</span></span>
          </button>
        </div>
        <div
          v-if="pathSuggestMatches.total > pathSuggestMatches.list.length"
          class="disk-path-suggest-more"
        >
          {{ tr("匹配") }} {{ pathSuggestMatches.total }} {{ tr("项，仅列出前") }} {{ pathSuggestMatches.list.length }} {{ tr("项") }}
        </div>
      </div>
      <div v-else-if="pathSuggestLoadingVisible" class="disk-path-suggest-loading">
        <Loader2 :size="11" class="spin" />
        <span>{{ tr("正在读取该层目录…") }}</span>
      </div>
      <div class="disk-filter-actions">
        <button
          class="ui-btn sm"
          :disabled="!filters[filterPopover.key]"
          @click="clearPopoverFilter"
        >{{ tr("清除") }}</button>
      </div>
    </template>
  </div></UiTransition>
</Teleport>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { useId } from 'vue';
import { Filter, X, CalendarDays, ChevronsLeft, ChevronLeft, ChevronRight, ChevronsRight, Folder, File as FileIcon, Loader2 } from 'lucide-vue-next';
import UiSelect from '../../../components/ui/UiSelect.vue';
import UiTransition from '../../../components/ui/UiTransition.vue';
import { BOOL_FILTER_OPTIONS } from '../model/model';
import { CAL_HOURS, CAL_MINSEC } from '../composables/useTimeFilters';
import type { useFileFilters } from '../composables/useFileFilters';

const props = defineProps<{ controls: ReturnType<typeof useFileFilters> }>();
const fieldId = useId();
const { filterPopover, filterPopoverColumn, hideFilterPopover, sizeFilter, sizeFilterErrors, timeFilterErrors,
  onSizeFormEnter, onFilterCloseKey, applySizeFilter, disableSizeFilter, SIZE_UNIT_OPTIONS,
  popoverTimeFilter, onTimeFormEnter, timeCalOpen, timeCalCursor, timeCalTime, timeCalCells,
  toggleTimeCal, moveTimeCal, pickCalTime, pickTimeCalDay, pickTimeCalToday, clearTimeCalRow,
  applyPopoverTimeFilter, disablePopoverTimeFilter, filters, onTextFilterInput, onTextFilterFocus, onTextFilterKeydown,
  pathSuggestVisible, pathSuggestMatches, pathSuggestLoadingVisible, pathSuggestIndex,
  onPathSuggestClick, clearPopoverFilter } = props.controls;
</script>

<style scoped>
/* ── 列头漏斗过滤气泡（Teleport 到 body；外观来自 ui-menu）── */
.disk-filter-popover {
  position: fixed;
  z-index: 9000;
  transform-origin: top center;
  padding: 10px 12px 12px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  font-family: var(--font-ui);
  border-radius: var(--radius-lg);
}

.disk-filter-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-right: -4px;
}

.disk-filter-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary);
}

/* ── 大小/时间列的结构化过滤表单（行布局复用同一套 disk-size-* 类）── */
.disk-size-form {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.disk-size-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.disk-size-row input[type='checkbox'] {
  margin: 0;
  flex-shrink: 0;
  cursor: pointer;
}

.disk-size-op {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--text-secondary);
  min-width: 26px;
  text-align: center;
  flex-shrink: 0;
}

.disk-size-num {
  flex: 1;
  text-align: right;
}

.disk-size-unit {
  width: 68px;
  flex-shrink: 0;
}

/* ── 时间列的范围过滤输入 ── */
.disk-time-wrap {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
}

.disk-time-input {
  flex: 1;
  padding-right: 28px;
}

/* 隐形的 datetime-local 已移除——日历改为自绘面板（见 .disk-time-cal）。 */
.disk-time-pick {
  position: absolute;
  right: 2px;
  --ui-icon-btn-size: 22px;
}

/* ── 自绘月历 + 时间轮（颜色全部走主题 token，三套主题自动同步）──
   日历格与时间轮单元共用 --cal-cell-w/h，保证两边长宽高严格对齐。 */
.disk-time-cal {
  --cal-cell-w: 34px;
  --cal-cell-h: 26px;
  display: flex;
  align-items: stretch;
  justify-content: space-between;
  gap: 8px;
  padding: 8px;
  background: var(--surface-primary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
}

.cal-main {
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.disk-time-cal-head {
  display: flex;
  align-items: center;
  gap: 2px;
  height: var(--cal-cell-h);
}

.cal-title {
  flex: 1;
  text-align: center;
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary);
}

.cal-nav {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 3px;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  cursor: pointer;
  transition:
    color var(--transition-fast),
    background var(--transition-fast);
}

.cal-nav:hover {
  color: var(--primary-color);
  background: var(--hover-bg);
}

.disk-time-cal-grid {
  display: grid;
  grid-template-columns: repeat(7, var(--cal-cell-w));
  gap: 2px;
}

.cal-weekday {
  display: flex;
  align-items: center;
  justify-content: center;
  height: var(--cal-cell-h);
  font-size: 11px;
  color: var(--text-secondary);
  user-select: none;
}

.cal-day {
  display: flex;
  align-items: center;
  justify-content: center;
  width: var(--cal-cell-w);
  height: var(--cal-cell-h);
  padding: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  cursor: pointer;
  transition:
    background var(--transition-fast),
    color var(--transition-fast);
}

.cal-day:hover {
  background: var(--hover-bg);
}

.cal-day.out {
  color: var(--text-secondary);
  opacity: 0.5;
}

.cal-day.today {
  box-shadow: inset 0 0 0 1px var(--primary-color);
}

.cal-day.selected {
  background: var(--primary-solid);
  color: var(--accent-contrast);
}

.disk-time-cal-foot {
  display: flex;
  justify-content: space-between;
  align-items: center;
  height: var(--cal-cell-h);
}

/* 仿原生选择器右侧的 时/分/秒 滚动选值列；单元尺寸与日历格一致。 */
.cal-time-cols {
  display: flex;
  gap: 2px;
  min-height: 0;
}

.cal-time-col {
  position: relative;
  width: var(--cal-cell-w);
  /* 列高 = 日历区总高（头 26 + gap 4 + 网格 7×26+12 + gap 4 + 脚 26 = 254px），
     即 9 行单元 + 20px 间距。不能用 height:100%——列内容会反过来把
     flex 行撑高，面板被拉到全部 24/60 项的高度，滚动失效。 */
  height: calc(var(--cal-cell-h) * 9 + 20px);
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  /* 原生时间轮无可见滚动条；Windows 滚动条会占掉列宽破坏与日历格的对齐。 */
  scrollbar-width: none;
}

.cal-time-col::-webkit-scrollbar {
  display: none;
}

.cal-time-item {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: var(--cal-cell-h);
  padding: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  cursor: pointer;
  transition:
    background var(--transition-fast),
    color var(--transition-fast);
}

.cal-time-item:hover {
  background: var(--hover-bg);
}

.cal-time-item.selected {
  background: var(--primary-solid);
  color: var(--accent-contrast);
}

/* ── bool 列（已删除）的单选过滤 ── */
.disk-bool-options {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 2px 0;
}

.disk-filter-input-row {
  position: relative;
  display: flex;
  align-items: center;
}

.disk-filter-funnel {
  position: absolute;
  left: 8px;
  color: var(--text-secondary);
  pointer-events: none;
}

.disk-filter-input {
  padding-left: 26px;
}

/* ── 全路径列的自动补全候选 ── */
.disk-path-suggest {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.disk-path-suggest-list {
  position: relative; /* 键盘导航用 offsetTop 手动滚动，需要自身是定位参照 */
  max-height: 196px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  padding: 2px;
  background: var(--surface-primary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
}

.disk-path-suggest-item {
  display: flex;
  align-items: center;
  gap: 6px;
  min-height: 26px;
  padding: 0 8px;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  cursor: pointer;
  text-align: left;
  min-width: 0;
}

.disk-path-suggest-item.active {
  background: var(--sidebar-active-bg);
}

.disk-path-suggest-ico {
  flex-shrink: 0;
  color: var(--text-secondary);
}

.disk-path-suggest-text {
  font-family: var(--font-mono);
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}

.disk-path-suggest-hit {
  font-weight: 600;
  color: var(--primary-color);
}

.disk-path-suggest-slash {
  color: var(--text-secondary);
}

.disk-path-suggest-more {
  font-size: 11px;
  color: var(--text-secondary);
  padding: 0 2px;
}

.disk-path-suggest-loading {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--text-secondary);
  padding: 2px;
}

.disk-filter-actions {
  display: flex;
  justify-content: flex-end;
  gap: 6px;
}
</style>
