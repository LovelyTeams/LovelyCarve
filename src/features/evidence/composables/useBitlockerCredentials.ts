
import { tr } from '../../../composables/useLocale';
import { computed, ref, useId } from 'vue';
import { checkRecoveryKey } from '../model/bitlockerRecoveryKey';

export function useBitlockerCredentials() {
  const passwordInput = ref('');
  const recoveryInput = ref('');
  const passwordEl = ref<HTMLInputElement | null>(null);
  const recoveryEl = ref<HTMLInputElement | null>(null);
  const credentialElements = { password: passwordEl, recovery: recoveryEl };

  const canSubmitPassword = computed(() => passwordInput.value.length > 0);

  // Recovery key: a finished group that fails the checksum is flagged as soon as
  // it is typed; what is merely missing waits until the user submits or leaves
  // the field, so a half-typed key never reads as an error.
  const recoveryCheck = computed(() => checkRecoveryKey(recoveryInput.value, tr));
  const recoveryTouched = ref(false);
  const recoveryNoteId = useId();
  const recoveryError = computed(() => recoveryCheck.value.groupError
    ?? (recoveryTouched.value && recoveryInput.value.trim() ? recoveryCheck.value.formatError : null));
  const canSubmitRecovery = computed(() => !recoveryCheck.value.groupError && !recoveryCheck.value.formatError);

  /** Leaving the field shows the canonical grouping and any missing part. */
  function onRecoveryBlur() {
    if (!recoveryInput.value.trim()) return;
    recoveryInput.value = recoveryCheck.value.normalized;
    recoveryTouched.value = true;
  }

  function clearCredentials() { passwordInput.value = ''; recoveryInput.value = ''; recoveryTouched.value = false; }
  return { passwordInput, recoveryInput, passwordEl, recoveryEl, credentialElements, canSubmitPassword, recoveryCheck, recoveryTouched, recoveryNoteId, recoveryError, canSubmitRecovery, onRecoveryBlur, clearCredentials };
}
