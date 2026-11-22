//! Running one command where the evidence is held: confirmation, progress,
//! the command itself and the verdict on its result. The same steps run in a
//! one-shot process, the shell, a batch and a case backend.
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    background::Background,
    cmd::{Command, Meta},
    progress::Reporter,
    prompt,
    workspace::Workspace,
    CliError, Output,
};
use crate::{
    commands::filesystem::filesystem_page::FsNodePage,
    core::{common::ids::ImageId, filesystem::evidence_session::EvidenceSession},
    host::AppHandle,
    state::AppState,
};

/// Where a command runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// One command, then the process exits.
    OneShot,
    /// The interactive shell or a shell script.
    Shell,
    /// JSONL requests on stdin.
    Batch,
    /// A case backend serving other processes.
    Backend,
}

#[derive(Clone)]
pub struct Exec {
    pub host: AppHandle,
    pub state: Arc<AppState>,
    pub mode: Mode,
    pub reporter: Reporter,
    pub background: Background,
    pub workspace: Workspace,
}

/// What applies to one request besides the command itself.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestOptions {
    /// Display time zone of file times.
    #[serde(default)]
    pub tz: Option<String>,
    /// A destructive command was confirmed.
    #[serde(default)]
    pub yes: bool,
}

/// A command's result. Where it is printed, `Meta::incomplete` judges
/// whether it is complete.
#[derive(Debug)]
pub struct Executed {
    pub output: Output,
}

/// A lookup whose subject must exist fails instead of returning `null`.
fn found(meta: &Meta, output: Output) -> Result<Output, CliError> {
    match (meta.missing, &output) {
        (Some(message), Output::Json(json)) if json.is_null() => {
            Err(CliError::new("notFound", message))
        }
        _ => Ok(output),
    }
}

impl Exec {
    /// Run `command` through every step.
    pub async fn run(
        &self,
        command: Command,
        options: &RequestOptions,
    ) -> Result<Executed, CliError> {
        let foreground = self.background.foreground();
        // On the heap: batch requests run on blocking threads with the
        // default stack, and the command futures are large.
        let executed = Box::pin(self.run_steps(command, options)).await;
        foreground.finish(&executed);
        executed
    }

    async fn run_steps(
        &self,
        command: Command,
        options: &RequestOptions,
    ) -> Result<Executed, CliError> {
        let meta = command.meta();
        let path = command.path();
        if let Some(zone) = options.tz.as_deref() {
            super::timezone::validate(zone)?;
        }
        if meta.lasting && self.mode == Mode::OneShot {
            return Err(CliError::new(
                "needsBackend",
                format!(
                    "{path} 的结果只在进程运行期间有效，一次性命令结束就会丢失：先用 case open 启动案件后台进程，\
                     或在 shell / batch 中运行"
                ),
            ));
        }
        if let Some(question) = meta.confirm {
            if !options.yes {
                match self.mode {
                    Mode::OneShot | Mode::Shell => {
                        if !prompt::confirm_delete(false, Some(question))? {
                            eprintln!("Aborted.");
                            return Err(CliError::aborted());
                        }
                    }
                    Mode::Batch | Mode::Backend => {
                        return Err(CliError::new(
                            "aborted",
                            format!("{path} 需要确认：{question}确认后加 -y"),
                        ))
                    }
                }
            }
        }
        let mut monitor = (self.mode != Mode::Batch).then(|| self.reporter.start(path));
        let executed = super::cmd::run(self, command, options)
            .await
            .and_then(|output| found(&meta, output))
            .map(|output| Executed { output });
        if let Some(monitor) = &mut monitor {
            monitor.finish(&executed);
        }
        executed
    }

    /// An open image.
    pub async fn image(&self, image_id: &ImageId) -> Result<Arc<EvidenceSession>, CliError> {
        self.state
            .images
            .lock()
            .await
            .get(image_id)
            .cloned()
            .ok_or_else(|| CliError::new("noImage", format!("检材 {image_id} 已关闭")))
    }

    /// One page of a directory, index first.
    pub async fn page(
        &self,
        image_id: &ImageId,
        parent_id: String,
        cursor: Option<String>,
        page_size: usize,
    ) -> Result<FsNodePage, CliError> {
        crate::commands::filesystem::filesystem::fs_children_page(
            crate::host::State::new(&self.state),
            image_id.clone(),
            parent_id.into(),
            cursor,
            Some(page_size),
            None,
        )
        .await
        .map_err(|error| super::command_error(error).in_operation("fs_children_page"))
    }
}
