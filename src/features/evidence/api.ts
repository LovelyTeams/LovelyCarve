export { useFileSession, registerEncryptedUnlockNavigator } from './composables/useFileSession';
export type { OpenFileOutcome } from './composables/useFileSession';
export { useAutoMount } from './composables/useAutoMount';
export { useImageExport } from './composables/useImageExport';
export { ensureAutoDetectListener, registerAutoDetectReportPublished, disposeAutoDetect } from './composables/useAutoDetect';

export { disposeBitlockerMemoryScans } from './composables/useBitlockerMemoryScan';

export { disposeImageHashJobs } from './composables/useImageHash';

export { registerEvidenceContext } from './composables/evidenceContext';
export { disposeImageExportJobs } from './composables/useImageExport';
export { registerEncryptedVolumeImageUpdater, registerEncryptedVolumeImageOpener, disposeEncryptedVolumeUnlockJobs } from './composables/useEncryptedVolumeUnlocks';
