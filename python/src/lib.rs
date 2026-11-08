//! Read-only bindings. No AppState, host initialization, CLI or command dispatch.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use lovelycarve_core::{
    evidence::image::DiskImage,
    filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{children_for_mount, file_stat, FsSession},
    },
};
use pyo3::{
    exceptions::{PyOSError, PyRuntimeError, PyValueError},
    prelude::*,
    types::PyBytes,
};
use serde::Serialize;

// Bounds a single native allocation; Python's file object can stream larger reads.
const MAX_READ: usize = 64 * 1024 * 1024;

#[pyclass(frozen, module = "lovelycarve._native", name = "Image")]
struct Image {
    session: Mutex<Option<Arc<EvidenceSession>>>,
}

impl Image {
    fn session(&self) -> PyResult<Arc<EvidenceSession>> {
        self.session
            .lock()
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?
            .as_ref()
            .cloned()
            .ok_or_else(|| PyValueError::new_err("I/O operation on closed image"))
    }
}

fn io_error(error: anyhow::Error) -> PyErr {
    PyOSError::new_err(format!("{error:#}"))
}

fn metadata_json(value: impl Serialize) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&value)?)
}

/// A tolerant backend must never deliver fabricated bytes as authentic Python data.
fn strict_read<T>(
    session: &EvidenceSession,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let (result, faults) = session.collect_read_faults(operation);
    let result = result?;
    anyhow::ensure!(
        !faults.read_faults_detected,
        "source read faults: {} synthetic bytes in {:?}",
        faults.synthetic_bytes,
        faults.bad_regions
    );
    Ok(result)
}

fn check_length(bytes: Vec<u8>, expected: usize) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        bytes.len() == expected,
        "unexpected short read: expected {expected} bytes, received {}",
        bytes.len()
    );
    Ok(bytes)
}

fn check_request(length: usize) -> PyResult<()> {
    if length > MAX_READ {
        return Err(PyValueError::new_err(
            "a single range read is limited to 64 MiB; use Image.open() for streaming",
        ));
    }
    Ok(())
}

#[pymethods]
impl Image {
    #[new]
    fn new(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        let session = py
            .detach(move || {
                let image = DiskImage::open(path)?;
                anyhow::ensure!(
                    image.bad_regions().is_empty(),
                    "source read faults encountered while opening the image"
                );
                Ok::<_, anyhow::Error>(Arc::new(EvidenceSession::from(image)))
            })
            .map_err(io_error)?;
        Ok(Self {
            session: Mutex::new(Some(session)),
        })
    }

    #[getter]
    fn closed(&self) -> PyResult<bool> {
        Ok(self
            .session
            .lock()
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?
            .is_none())
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let session = self
            .session
            .lock()
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?
            .take();
        py.detach(move || drop(session));
        Ok(())
    }

    fn info_json(&self, py: Python<'_>) -> PyResult<String> {
        let session = self.session()?;
        py.detach(move || metadata_json(session.info()))
            .map_err(io_error)
    }

    fn children_json(&self, py: Python<'_>, node_id: String) -> PyResult<String> {
        let session = self.session()?;
        py.detach(move || {
            strict_read(&session, || {
                metadata_json(children_for_mount(&session, &node_id)?)
            })
        })
        .map_err(io_error)
    }

    fn stat_json(&self, py: Python<'_>, node_id: String) -> PyResult<String> {
        let session = self.session()?;
        py.detach(move || strict_read(&session, || metadata_json(file_stat(&session, &node_id)?)))
            .map_err(io_error)
    }

    fn read<'py>(
        &self,
        py: Python<'py>,
        offset: u64,
        length: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let session = self.session()?;
        check_request(length)?;
        let bytes = py
            .detach(move || {
                strict_read(&session, || {
                    let expected = session.len().saturating_sub(offset).min(length as u64) as usize;
                    check_length(session.read_range(offset, expected)?, expected)
                })
            })
            .map_err(io_error)?;
        Ok(PyBytes::new(py, &bytes))
    }

    fn read_file<'py>(
        &self,
        py: Python<'py>,
        node_id: String,
        offset: u64,
        length: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let session = self.session()?;
        check_request(length)?;
        let bytes = py
            .detach(move || {
                strict_read(&session, || {
                    let mut reader = FsSession::new(&session);
                    let size = reader.file_stat(&node_id)?.size;
                    let expected = size.saturating_sub(offset).min(length as u64) as usize;
                    if expected == 0 {
                        return Ok(Vec::new());
                    }
                    check_length(
                        reader.read_file_range(&node_id, offset, expected)?,
                        expected,
                    )
                })
            })
            .map_err(io_error)?;
        Ok(PyBytes::new(py, &bytes))
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Image>()?;
    module.add("MAX_READ", MAX_READ)?;
    Ok(())
}
