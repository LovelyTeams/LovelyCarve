<template>
  <Transition name="ui-modal">
    <div v-if="visible" class="ui-modal-overlay" @click.self="$emit('close')">
      <div ref="dialogEl" class="ui-modal tool-settings-modal" role="dialog" aria-modal="true"
        aria-labelledby="tool-settings-title" tabindex="-1" @keydown.esc.stop="$emit('close')">
        <header class="ui-modal-header">
          <span class="ui-modal-icon"><Settings :size="18" aria-hidden="true" /></span>
          <div class="ui-modal-heading">
            <h2 id="tool-settings-title" class="ui-modal-title">{{ tr("设置中心") }}</h2>
          </div>
          <button class="ui-icon-btn" type="button" :title="tr('关闭')" :aria-label="tr('关闭设置')" @click="$emit('close')"><X :size="16" /></button>
        </header>

        <nav class="settings-nav" :aria-label="tr('设置分类')">
          <button v-for="section in sections" :key="section.id" type="button" class="ui-btn ghost settings-nav-item"
            :class="{ active: activeSection === section.id }" :aria-current="activeSection === section.id ? 'page' : undefined"
            @click="activeSection = section.id">
            <component :is="section.icon" :size="14" aria-hidden="true" /><strong>{{ section.label }}</strong>
          </button>
        </nav>

        <main class="ui-modal-body settings-content" v-content-motion.rise="activeSection">
          <PluginSettingsPanel v-if="activeSection === 'plugins'" />
          <PluginMarketPanel v-else-if="activeSection === 'market'" :install-link="pluginInstallLink"
            @consume-install-link="$emit('consumePluginInstall')" />
          <AppLogPanel v-else-if="activeSection === 'logs'" />
          <AppAboutPanel v-else-if="activeSection === 'about'" :updates="updates" />
          <section v-else class="settings-panel" :aria-label="tr('界面设置')">
            <div class="setting-row">
              <h3 class="setting-label" :id="languageLabelId">{{ tr('语言') }}</h3>
              <div class="setting-control">
                <div class="ui-seg" role="radiogroup" :aria-labelledby="languageLabelId">
                  <label v-for="option in languageOptions" :key="option.value" class="ui-seg-item" :class="{ active: locale === option.value }">
                    <input type="radio" class="sr-only" :name="languageLabelId" :value="option.value"
                      :checked="locale === option.value" @change="setLocale(option.value)" />{{ option.label }}
                  </label>
                </div>
              </div>
            </div>
            <div class="setting-row">
              <h3 class="setting-label" :id="themeLabelId">{{ tr("主题") }}</h3>
              <div class="ui-seg theme-options" role="group" :aria-labelledby="themeLabelId">
                <button v-for="theme in themes" :key="theme" class="ui-seg-item" type="button"
                  :class="{ active: currentTheme === theme }" :aria-pressed="currentTheme === theme"
                  @click="setTheme(theme, $event.currentTarget as Element)">
                  <component :is="themeIcons[theme]" :size="14" aria-hidden="true" />{{ themeNames[theme] }}
                </button>
              </div>
            </div>

            <div class="setting-row">
              <h3 class="setting-label" :id="motionLabelId">{{ tr("界面动画") }}</h3>
              <div class="setting-control">
                <div class="ui-seg" role="radiogroup" :aria-labelledby="motionLabelId">
                  <label v-for="option in motionOptions" :key="option.value" class="ui-seg-item" :class="{ active: motionPreference === option.value }">
                    <input v-model="motionPreference" type="radio" class="sr-only" :name="motionLabelId" :value="option.value" />{{ option.label }}
                  </label>
                </div>
              </div>
            </div>

            <div class="setting-row mount-setting">
              <h3 class="setting-label" :id="mountLabelId">{{ tr("分区挂载") }}</h3>
              <div class="setting-control">
                <div class="ui-seg" role="radiogroup" :aria-labelledby="mountLabelId">
                  <label class="ui-seg-item" :class="{ active: mountTarget === 'drive' }">
                    <input type="radio" class="sr-only" :name="mountLabelId" value="drive" v-model="mountTarget" />
                    <HardDrive :size="14" aria-hidden="true" />{{ tr("驱动器盘符") }}
                  </label>
                  <label class="ui-seg-item" :class="{ active: mountTarget === 'folder' }">
                    <input type="radio" class="sr-only" :name="mountLabelId" value="folder" v-model="mountTarget" />
                    <FolderOpen :size="14" aria-hidden="true" />{{ tr("文件夹") }}
                  </label>
                </div>
                <div v-if="mountTarget === 'folder'" class="mount-options">
                  <label class="setting-label" :for="mountRootId">{{ tr("默认根目录") }}</label>
                  <div class="path-row">
                    <input :id="mountRootId" v-model="mountFolderRoot" type="text" class="ui-input mono path-input"
                      :placeholder="activeCase?.dir || tr('留空使用案件目录')" />
                    <button class="ui-btn" type="button" :disabled="pickingDirectory" @click="pickMountFolderRoot">
                      <Loader2 v-if="pickingDirectory" :size="14" class="spin" /><FolderOpen v-else :size="14" aria-hidden="true" />{{ tr("选择") }}
                    </button>
                  </div>
                  <label class="auto-mount-row">
                    <strong>{{ tr("打开案件时自动挂载") }}</strong>
                    <input type="checkbox" role="switch" class="ui-switch" v-model="autoMountCaseEvidence" :aria-label="tr('打开案件时自动挂载全部分区')" />
                  </label>
                </div>
              </div>
            </div>
            <p v-if="directoryError" class="ui-alert danger" role="alert">{{ directoryError }}</p>
          </section>
        </main>
        <footer class="ui-modal-footer">
          <button v-if="activeSection === 'appearance'" class="ui-btn ghost start" type="button" @click="resetCurrentSection">{{ tr("恢复本页默认设置") }}</button>
          <button class="ui-btn primary" type="button" @click="$emit('close')">{{ tr("完成") }}</button>
        </footer>
      </div>
    </div>
  </Transition>
</template>

<script setup lang="ts">
import { tr } from '../../../composables/useLocale';
import { useLocale, DEFAULT_LOCALE, type Locale } from '../../../composables/useLocale';

import { vContentMotion } from '../../../composables/uiMotionDirectives';
import { markRaw, ref, useId, watch, type Component } from 'vue';
import {
  FolderOpen, HardDrive, Info, Loader2, Moon, Palette, Puzzle, Settings, Store, Sun, Terminal, X,
} from 'lucide-vue-next';
import { open } from '@tauri-apps/plugin-dialog';
import { useUiPreferences, type MotionPreference } from '../../../composables/useUiPreferences';
import { useCaseContext } from '../../cases/api';
import { useDialogFocus } from '../../../composables/useDialogFocus';
import { useTheme, type ThemeName } from '../../../composables/useTheme';
import type { SettingsSection } from '../model/types';
import type { PluginInstallLink } from '../../../lib/ipc/evidenceLinks';
import PluginSettingsPanel from './PluginSettingsPanel.vue';
import PluginMarketPanel from './PluginMarketPanel.vue';
import AppLogPanel from './AppLogPanel.vue';
import AppAboutPanel from './AppAboutPanel.vue';
import { useAppUpdateCheck } from '../composables/useAppUpdateCheck';

const updates = useAppUpdateCheck();

const props = withDefaults(defineProps<{
  visible?: boolean;
  initialSection?: SettingsSection;
  pluginInstallLink?: PluginInstallLink | null;
}>(), {
  visible: false,
  initialSection: 'appearance',
});

defineEmits<{
  close: [];
  consumePluginInstall: [];
}>();

const sections = [
  { id: 'appearance' as const, get label() { return tr("界面"); }, icon: markRaw(Palette) },
  { id: 'plugins' as const, get label() { return tr("插件"); }, icon: markRaw(Puzzle) },
  { id: 'market' as const, get label() { return tr("插件市场"); }, icon: markRaw(Store) },
  { id: 'logs' as const, get label() { return tr("日志"); }, icon: markRaw(Terminal) },
  { id: 'about' as const, get label() { return tr("关于"); }, icon: markRaw(Info) },
];

const activeSection = ref<SettingsSection>(props.initialSection);

watch(
  () => [props.visible, props.initialSection, props.pluginInstallLink] as const,
  ([visible, section, installLink]) => {
    if (visible) activeSection.value = installLink ? 'market' : section;
  },
);

const dialogEl = ref<HTMLElement | null>(null);
useDialogFocus(() => props.visible, dialogEl);
const themeLabelId = useId();
const languageLabelId = useId();
const { locale, setLocale } = useLocale();
const languageOptions: { value: Locale; label: string }[] = [
  { value: 'zh-CN', label: '简体中文' },
  { value: 'en', label: 'English' },
];
const motionLabelId = useId();
const mountLabelId = useId();
const mountRootId = useId();
const pickingDirectory = ref(false);
const directoryError = ref('');

const { motionPreference, mountTarget, mountFolderRoot, autoMountCaseEvidence } = useUiPreferences();
const { currentTheme, setTheme, themes } = useTheme();
const themeIcons: Record<ThemeName, Component> = { light: Sun, dark: Moon };
const themeNames: Record<ThemeName, string> = { get light() { return tr("浅色"); }, get dark() { return tr("深色"); } };

const motionOptions: { value: MotionPreference; label: string }[] = [
  { value: 'system', get label() { return tr("跟随系统"); } },
  { value: 'on', get label() { return tr("开启"); } },
  { value: 'off', get label() { return tr('motion.off'); } },
];
const { activeCase } = useCaseContext();

async function pickMountFolderRoot() {
  if (pickingDirectory.value) return;
  pickingDirectory.value = true;
  directoryError.value = '';
  try {
    const result = await open({
      multiple: false,
      directory: true,
      defaultPath: mountFolderRoot.value.trim() || activeCase.value?.dir || undefined,
      title: tr("选择默认挂载根目录"),
    });
    if (typeof result === 'string') mountFolderRoot.value = result;
  } catch (cause) {
    directoryError.value = tr("选择目录失败：{0}。请重试。", { 0: String(cause) });
  } finally {
    pickingDirectory.value = false;
  }
}

function resetCurrentSection() {
  if (activeSection.value === 'appearance') {
    setLocale(DEFAULT_LOCALE);
    setTheme('light');
    motionPreference.value = 'system';
    mountTarget.value = 'drive';
    mountFolderRoot.value = '';
    autoMountCaseEvidence.value = false;
  }
}
</script>

<style scoped>
.tool-settings-modal { width: min(760px, 100%); container: settings / inline-size; }
.settings-nav { display: flex; flex-wrap: wrap; gap: 8px; padding: 4px 20px 8px; border-bottom: 1px solid var(--border-color); }
.settings-nav-item strong { font-weight: 500; }
.settings-content { padding: 20px; }
.settings-panel { display: flex; flex-direction: column; gap: 24px; }
.theme-options { justify-self: start; }
.setting-row { display: grid; grid-template-columns: 100px minmax(0, 1fr); align-items: start; gap: 16px; }
.setting-label { margin: 0; padding-top: 4px; font-size: 13px; font-weight: 600; line-height: 1.5; }
.setting-control { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; min-width: 0; }
.mount-setting { padding-top: 20px; border-top: 1px solid var(--border-color); }
.mount-options { display: flex; flex-direction: column; gap: 8px; width: 100%; margin-top: 8px; }
.mount-options > .setting-label { font-size: 12px; }
.path-row { display: flex; align-items: center; gap: 8px; }
.path-input { flex: 1; min-width: 0; }
.auto-mount-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding-top: 12px; cursor: pointer; }
.auto-mount-row strong { font-size: 12px; font-weight: 500; }
@container settings (max-width: 520px) {
  .setting-row { grid-template-columns: 1fr; gap: 8px; }
  .settings-content { padding: 16px; }
}
</style>
