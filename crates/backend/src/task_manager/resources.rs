//! Resource lanes for long-running backend tasks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::{TaskKind, TaskRecord, TaskResourceAcquireError};

const DEFAULT_GLOBAL_BACKGROUND_LIMIT: usize = 2;
const DEFAULT_IMAGE_SEQUENTIAL_IO_LIMIT: usize = 1;
const DEFAULT_IMAGE_RANDOM_IO_LIMIT: usize = 2;
const DEFAULT_GPU_TASK_LIMIT: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskResourceClass {
    GlobalBackground,
    ImageSequentialIo,
    ImageRandomIo,
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageIoProfile {
    Sequential,
    Random,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TaskResourceProfile {
    global_background: bool,
    image_io: Option<ImageIoProfile>,
    cpu: bool,
    gpu: bool,
}

impl TaskResourceProfile {
    const fn background_sequential_io() -> Self {
        Self {
            global_background: true,
            image_io: Some(ImageIoProfile::Sequential),
            cpu: false,
            gpu: false,
        }
    }

    const fn background_random_io() -> Self {
        Self {
            global_background: true,
            image_io: Some(ImageIoProfile::Random),
            cpu: false,
            gpu: false,
        }
    }

    const fn background_cpu() -> Self {
        Self {
            global_background: true,
            image_io: None,
            cpu: true,
            gpu: false,
        }
    }

    const fn background_random_io_cpu() -> Self {
        Self {
            global_background: true,
            image_io: Some(ImageIoProfile::Random),
            cpu: true,
            gpu: false,
        }
    }

    #[cfg(feature = "ocr-cuda")]
    const fn background_random_io_gpu() -> Self {
        Self {
            global_background: true,
            image_io: Some(ImageIoProfile::Random),
            cpu: false,
            gpu: true,
        }
    }

    /// Short, bounded probes start at once. Their reads are as small as
    /// interactive browsing, and queueing them behind hashes or snapshot
    /// builds would hold back a result the UI is waiting for.
    const fn unbudgeted() -> Self {
        Self {
            global_background: false,
            image_io: None,
            cpu: false,
            gpu: false,
        }
    }

    fn classes(self) -> Vec<TaskResourceClass> {
        let mut classes = Vec::new();
        if self.global_background {
            classes.push(TaskResourceClass::GlobalBackground);
        }
        match self.image_io {
            Some(ImageIoProfile::Sequential) => classes.push(TaskResourceClass::ImageSequentialIo),
            Some(ImageIoProfile::Random) => classes.push(TaskResourceClass::ImageRandomIo),
            None => {}
        }
        if self.cpu {
            classes.push(TaskResourceClass::Cpu);
        }
        if self.gpu {
            classes.push(TaskResourceClass::Gpu);
        }
        classes
    }
}

impl TaskKind {
    pub(super) fn resource_classes(self) -> Vec<TaskResourceClass> {
        self.resource_profile().classes()
    }

    fn resource_profile(self) -> TaskResourceProfile {
        match self {
            // Browsing waits for a container's merged index.
            Self::AutoDetect | Self::DockerPrepare => TaskResourceProfile::unbudgeted(),
            Self::BitlockerExport
            | Self::BitlockerUnlock
            | Self::EncryptedVolumeUnlock
            | Self::CarveRecovery
            | Self::CarveScan
            | Self::DirectoryExport
            | Self::ImageExport
            | Self::ImageLoad
            | Self::ImageHash => TaskResourceProfile::background_sequential_io(),
            Self::FlatList
            | Self::MediaMaterialization
            | Self::SessionSnapshot
            | Self::ShadowCopyOpen
            | Self::VolumeSnapshot => TaskResourceProfile::background_random_io(),
            Self::BitlockerScan | Self::TimelineBuild => TaskResourceProfile::background_cpu(),
            Self::GlobalSearch
            | Self::IosBackupAudit
            | Self::IosBackupUnlock
            | Self::FilevaultInspect => TaskResourceProfile::background_random_io_cpu(),
            Self::QrScan => TaskResourceProfile::background_random_io_cpu(),
            Self::OcrScan => ocr_resource_profile(),
        }
    }
}

struct ImageSemaphorePool {
    limit: usize,
    semaphores: Mutex<HashMap<String, std::sync::Weak<Semaphore>>>,
}

impl ImageSemaphorePool {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            semaphores: Mutex::new(HashMap::new()),
        }
    }

    fn semaphore(&self, image_id: &str) -> Arc<Semaphore> {
        let mut semaphores = self
            .semaphores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        semaphores.retain(|_, semaphore| semaphore.strong_count() > 0);
        if let Some(semaphore) = semaphores.get(image_id).and_then(std::sync::Weak::upgrade) {
            return semaphore;
        }

        let semaphore = Arc::new(Semaphore::new(self.limit));
        semaphores.insert(image_id.to_string(), Arc::downgrade(&semaphore));
        semaphore
    }
}

pub(super) struct TaskResources {
    global_background: Arc<Semaphore>,
    image_sequential_io: ImageSemaphorePool,
    image_random_io: ImageSemaphorePool,
    cpu_tasks: Arc<Semaphore>,
    gpu_tasks: Arc<Semaphore>,
}

impl Default for TaskResources {
    fn default() -> Self {
        Self::new(
            DEFAULT_GLOBAL_BACKGROUND_LIMIT,
            DEFAULT_IMAGE_SEQUENTIAL_IO_LIMIT,
            DEFAULT_IMAGE_RANDOM_IO_LIMIT,
            default_cpu_task_limit(),
            DEFAULT_GPU_TASK_LIMIT,
        )
    }
}

impl TaskResources {
    pub(super) fn with_primary_limits(
        global_background_limit: usize,
        image_sequential_io_limit: usize,
    ) -> Self {
        Self::new(
            global_background_limit,
            image_sequential_io_limit,
            DEFAULT_IMAGE_RANDOM_IO_LIMIT,
            default_cpu_task_limit(),
            DEFAULT_GPU_TASK_LIMIT,
        )
    }

    pub(super) fn new(
        global_background_limit: usize,
        image_sequential_io_limit: usize,
        image_random_io_limit: usize,
        cpu_task_limit: usize,
        gpu_task_limit: usize,
    ) -> Self {
        Self {
            global_background: Arc::new(Semaphore::new(global_background_limit.max(1))),
            image_sequential_io: ImageSemaphorePool::new(image_sequential_io_limit),
            image_random_io: ImageSemaphorePool::new(image_random_io_limit),
            cpu_tasks: Arc::new(Semaphore::new(cpu_task_limit.max(1))),
            gpu_tasks: Arc::new(Semaphore::new(gpu_task_limit.max(1))),
        }
    }

    pub(super) async fn acquire(
        &self,
        record: &TaskRecord,
    ) -> Result<TaskResourcePermit, TaskResourceAcquireError> {
        let profile = record.kind().resource_profile();
        let token = record.token.clone();

        let global_background = if profile.global_background {
            Some(
                acquire_owned_cancelable(
                    Arc::clone(&self.global_background),
                    &token,
                    "global background task",
                )
                .await?,
            )
        } else {
            None
        };

        let mut image_sequential_io = None;
        let mut image_random_io = None;
        match (profile.image_io, record.image_id()) {
            (Some(ImageIoProfile::Sequential), Some(image_id)) => {
                let semaphore = self.image_sequential_io.semaphore(image_id);
                let permit =
                    acquire_owned_cancelable(semaphore, &token, "per-image sequential I/O").await?;
                image_sequential_io = Some(permit);
            }
            (Some(ImageIoProfile::Random), Some(image_id)) => {
                let semaphore = self.image_random_io.semaphore(image_id);
                let permit =
                    acquire_owned_cancelable(semaphore, &token, "per-image random I/O").await?;
                image_random_io = Some(permit);
            }
            (Some(_), None) | (None, _) => {}
        }

        let cpu = if profile.cpu {
            Some(acquire_owned_cancelable(Arc::clone(&self.cpu_tasks), &token, "CPU task").await?)
        } else {
            None
        };
        let gpu = if profile.gpu {
            Some(acquire_owned_cancelable(Arc::clone(&self.gpu_tasks), &token, "GPU task").await?)
        } else {
            None
        };

        Ok(TaskResourcePermit {
            _global_background: global_background,
            _image_sequential_io: image_sequential_io,
            _image_random_io: image_random_io,
            _cpu: cpu,
            _gpu: gpu,
        })
    }
}

fn default_cpu_task_limit() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

#[cfg(feature = "ocr-cuda")]
const fn ocr_resource_profile() -> TaskResourceProfile {
    TaskResourceProfile::background_random_io_gpu()
}

#[cfg(not(feature = "ocr-cuda"))]
const fn ocr_resource_profile() -> TaskResourceProfile {
    TaskResourceProfile::background_random_io_cpu()
}

async fn acquire_owned_cancelable(
    semaphore: Arc<Semaphore>,
    token: &CancellationToken,
    label: &str,
) -> Result<OwnedSemaphorePermit, TaskResourceAcquireError> {
    tokio::select! {
        permit = semaphore.acquire_owned() => {
            permit.map_err(|_| TaskResourceAcquireError::SemaphoreClosed {
                label: label.to_string(),
            })
        }
        () = token.cancelled() => Err(TaskResourceAcquireError::Cancelled),
    }
}

pub struct TaskResourcePermit {
    _global_background: Option<OwnedSemaphorePermit>,
    _image_sequential_io: Option<OwnedSemaphorePermit>,
    _image_random_io: Option<OwnedSemaphorePermit>,
    _cpu: Option<OwnedSemaphorePermit>,
    _gpu: Option<OwnedSemaphorePermit>,
}
