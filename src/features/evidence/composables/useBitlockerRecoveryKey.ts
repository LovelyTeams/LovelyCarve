
import { tr } from '../../../composables/useLocale';
import { computed, ref, useId } from 'vue';
import { checkRecoveryKey } from '../model/bitlockerRecoveryKey';

/** The BitLocker 48-digit recovery password field of the unlock dialog. */
export function useBitlockerRecoveryKey() {
  const recoveryInput = ref('');

  // Recovery key: a finished group that fails the checksum is flagged as soon as
  // it is typed; what is merely missing waits until the user submits or leaves
  // the field, so a half-typed key never reads as an error.
  const recoveryCheck = computed(() => checkRecoveryKey(recoveryInput.value, tr));
  const recoveryTouched = ref(false);
  const recoveryNoteId = useId();
  const recoveryError = computed(() => recoveryCheck.value.groupError
    ?? (recoveryTouched.value && recoveryInput.value.trim() ? recoveryCheck.value.formatError : null));
  const canSubmitRecovery = computed(() => !!recoveryInput.value.trim() && !recoveryCheck.value.groupError && !recoveryCheck.value.formatError);

  /** Leaving the field shows the canonical grouping and any missing part. */
  function onRecoveryBlur() {
    if (!recoveryInput.value.trim()) return;
    recoveryInput.value = recoveryCheck.value.normalized;
    recoveryTouched.value = true;
  }

  function clearRecovery() { recoveryInput.value = ''; recoveryTouched.value = false; }
  return { recoveryInput, recoveryCheck, recoveryNoteId, recoveryError, canSubmitRecovery, onRecoveryBlur, clearRecovery };
}
