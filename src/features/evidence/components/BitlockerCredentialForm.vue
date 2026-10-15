<template>
  <!-- ── Password mode ───────────────────────────────────────────────── -->
  <template v-if="mode === 'password'">
    <input
      :ref="elements.password"
      v-model="passwordInput"
      class="ui-input mono bde-input"
      type="password"
      autocomplete="off"
      spellcheck="false"
      :placeholder="tr('BitLocker 用户密码')"
      @keydown.enter.prevent="emit('password')"
      @keydown.esc.prevent="emit('close')"
    />
    <div class="bde-actions">
      <button class="ui-btn sm" @click="emit('close')">{{ tr("取消") }}</button>
      <button
        class="ui-btn sm primary"
        :disabled="!canSubmitPassword"
        @click="emit('password')"
      >{{ tr("解锁") }}</button>
    </div>
  </template>

  <!-- ── Recovery-key mode ───────────────────────────────────────────── -->
  <template v-if="mode === 'recovery'">
    <input
      :ref="elements.recovery"
      v-model="recoveryInput"
      class="ui-input mono bde-input"
      type="text"
      autocomplete="off"
      spellcheck="false"
      placeholder="xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx-xxxxxx"
      :aria-invalid="recoveryError ? 'true' : undefined"
      :aria-describedby="recoveryNoteId"
      @blur="emit('blur')"
      @keydown.enter.prevent="emit('recovery')"
      @keydown.esc.prevent="emit('close')"
    />
    <p v-if="recoveryError" :id="recoveryNoteId" class="ui-field-error bde-field-note" role="alert">
      <AlertCircle :size="12" /><span>{{ recoveryError }}</span>
    </p>
    <p v-else :id="recoveryNoteId" class="ui-field-hint bde-field-note">
      {{ tr("共 8 组 6 位数字，可直接粘贴 · 已完成") }} <span class="mono">{{ validGroups }}/8</span> {{ tr("组") }}
    </p>
    <div class="bde-actions">
      <button class="ui-btn sm" @click="emit('close')">{{ tr("取消") }}</button>
      <button
        class="ui-btn sm primary"
        :disabled="!canSubmitRecovery"
        @click="emit('recovery')"
      >{{ tr("解锁") }}</button>
    </div>
  </template>
</template>
<script setup lang="ts">
import { tr } from '../../../composables/useLocale';

import { AlertCircle } from 'lucide-vue-next';
import type { Ref } from 'vue';
import type { BitlockerInputMode } from '../model/bitlocker';
defineProps<{ mode: BitlockerInputMode; elements: { password: Ref<HTMLInputElement | null>; recovery: Ref<HTMLInputElement | null> }; recoveryError: string | null; recoveryNoteId: string; validGroups: number; canSubmitPassword: boolean; canSubmitRecovery: boolean }>();
const emit = defineEmits<{ password: []; recovery: []; blur: []; close: [] }>();
const passwordInput = defineModel<string>('passwordInput', { required: true });
const recoveryInput = defineModel<string>('recoveryInput', { required: true });
</script>
<style scoped src="../styles/bitlocker-form.css"></style>
<style scoped>
.bde-input {
  height: 32px;
}

/* Hint / error sits close under the field it describes. */
.bde-field-note {
  margin: -4px 0 0;
}

</style>
