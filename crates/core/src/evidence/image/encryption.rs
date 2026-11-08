//! encryption implementation.
use super::*;

impl DiskImage {
    pub fn bitlocker_unlocked(&self, partition_id: u8) -> Option<Arc<UnlockedVolume>> {
        self.unlocked_bde
            .lock()
            .ok()
            .and_then(|map| map.get(&partition_id).cloned())
    }

    pub fn bitlocker_status(&self, partition_id: u8) -> anyhow::Result<BitlockerStatus> {
        let partition = self
            .partitions
            .iter()
            .find(|p| p.id == partition_id)
            .ok_or_else(|| anyhow!("partition not found: {partition_id}"))?;
        if partition.kind != PartitionKind::Bitlocker {
            anyhow::bail!("partition {partition_id} is not BitLocker");
        }
        if let Some(unlocked) = self.bitlocker_unlocked(partition_id) {
            return Ok(BitlockerStatus {
                locked: false,
                size_bytes: Some(unlocked.size()),
                encryption_method: unlocked.encryption_method().map(encryption_method_label),
                // The in-place unlock reads straight from the image, so there
                // is no temp dump path to surface.
                cached_dump: None,
            });
        }
        Ok(BitlockerStatus {
            locked: true,
            size_bytes: Some(partition.length_lba * self.sector_size as u64),
            encryption_method: None,
            cached_dump: None,
        })
    }

    pub fn unlock_bitlocker_partition(
        self: &Arc<Self>,
        partition_id: u8,
        recovery_password: Option<&str>,
        password: Option<&str>,
    ) -> anyhow::Result<BitlockerStatus> {
        self.unlock_bitlocker_partition_with_observer(
            partition_id,
            recovery_password,
            password,
            &bitlocker::UnlockObserver::noop(),
        )
    }

    /// Same as `unlock_bitlocker_partition` but threads unlock progress and
    /// cancellation through to `bitlocker::unlock_location` so the IPC
    /// layer can drive a progress bar and react to `task_cancel`.
    pub fn unlock_bitlocker_partition_with_observer(
        self: &Arc<Self>,
        partition_id: u8,
        recovery_password: Option<&str>,
        password: Option<&str>,
        observer: &bitlocker::UnlockObserver<'_>,
    ) -> anyhow::Result<BitlockerStatus> {
        let key = match (recovery_password, password) {
            (Some(value), _) if !value.trim().is_empty() => BdeUnlockKey::RecoveryPassword(value),
            (_, Some(value)) if !value.is_empty() => BdeUnlockKey::Password(value),
            _ => BdeUnlockKey::Clear,
        };
        self.unlock_bitlocker_key_with_observer(partition_id, key, observer)
    }

    pub fn unlock_bitlocker_key_with_observer(
        self: &Arc<Self>,
        partition_id: u8,
        key: BdeUnlockKey<'_>,
        observer: &bitlocker::UnlockObserver<'_>,
    ) -> anyhow::Result<BitlockerStatus> {
        let partition = self
            .partitions
            .iter()
            .find(|p| p.id == partition_id)
            .ok_or_else(|| anyhow!("partition not found: {partition_id}"))?
            .clone();
        if partition.kind != PartitionKind::Bitlocker {
            anyhow::bail!("partition {partition_id} is not BitLocker");
        }
        if self.bitlocker_unlocked(partition_id).is_some() {
            return self.bitlocker_status(partition_id);
        }
        let location = bitlocker::BitLockerLocation {
            partition_id: Some(partition_id),
            offset: partition
                .start_lba
                .checked_mul(self.sector_size as u64)
                .context("BitLocker partition offset overflow")?,
            length: partition
                .length_lba
                .checked_mul(self.sector_size as u64)
                .context("BitLocker partition size overflow")?,
        };
        let unlocked = bitlocker::unlock_location(self, location, key, observer)?;
        let unlocked = Arc::new(unlocked);
        self.unlocked_bde
            .lock()
            .map_err(|_| anyhow!("unlocked-volume registry poisoned"))?
            .insert(partition_id, unlocked);
        self.bitlocker_status(partition_id)
    }

    /// Derive the BitLocker 48-digit recovery key for `partition_id` from
    /// the supplied 16- or 32-byte VMK by replaying the FVE metadata's
    /// AES-CCM datum, then drive the regular recovery-password unlock
    /// path. Returns the resolved recovery key alongside the unlock
    /// status so the frontend can surface it for forensic logging.
    pub fn unlock_bitlocker_with_vmk(
        self: &Arc<Self>,
        partition_id: u8,
        vmk: &[u8],
    ) -> anyhow::Result<(BitlockerStatus, String)> {
        self.unlock_bitlocker_with_vmk_with_observer(
            partition_id,
            vmk,
            &bitlocker::UnlockObserver::noop(),
        )
    }

    /// Same as `unlock_bitlocker_with_vmk` but forwards unlock progress and
    /// cancellation to the underlying `unlock_location`.
    pub fn unlock_bitlocker_with_vmk_with_observer(
        self: &Arc<Self>,
        partition_id: u8,
        vmk: &[u8],
        observer: &bitlocker::UnlockObserver<'_>,
    ) -> anyhow::Result<(BitlockerStatus, String)> {
        let partition = self
            .partitions
            .iter()
            .find(|p| p.id == partition_id)
            .ok_or_else(|| anyhow!("partition not found: {partition_id}"))?
            .clone();
        if partition.kind != PartitionKind::Bitlocker {
            anyhow::bail!("partition {partition_id} is not BitLocker");
        }
        // Keep the legacy 128-bit recovery-datum path while allowing real 256-bit
        // VMKs to unwrap the FVEK without any recovery-password protector.
        if vmk.len() == 16 {
            let recovery = bde_vmk::recovery_key_from_vmk(self, &partition, vmk)?
                .ok_or_else(|| anyhow!("the supplied key does not unlock a recovery datum"))?;
            let status = self.unlock_bitlocker_partition_with_observer(
                partition_id,
                Some(&recovery),
                None,
                observer,
            )?;
            return Ok((status, recovery));
        }
        let status = self.unlock_bitlocker_key_with_observer(
            partition_id,
            BdeUnlockKey::Vmk(vmk),
            observer,
        )?;
        let recovery = match bde_vmk::recovery_key_from_vmk(self, &partition, vmk) {
            Ok(recovery) => recovery.unwrap_or_default(),
            Err(error) => {
                tracing::warn!(
                    "BitLocker unlocked; recovery-password extraction unavailable: {error:#}"
                );
                String::new()
            }
        };
        Ok((status, recovery))
    }
}
