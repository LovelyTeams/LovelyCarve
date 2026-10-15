//! Isolated plugin process startup and restricted environment.
use super::*;

/// Longest stderr line held before it is reported as one.
const MAX_STDERR_LINE: usize = 64 * 1024;

impl PluginClient {
    pub(in crate::core::extensions::plugin) async fn spawn(
        command: PluginCommand,
        services: HostRpcServices,
        sinks: PluginSinks,
    ) -> Result<Arc<Self>> {
        let mut process = Command::new(&command.program);
        process
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // The release app has no console; without CREATE_NO_WINDOW Windows
        // would open one for the console-subsystem loader.
        #[cfg(windows)]
        process.creation_flags(0x0800_0000);
        apply_restricted_environment(&mut process, &services.permissions);
        if let Some(working_directory) = &command.working_directory {
            process.current_dir(working_directory);
        }
        let mut child = process.spawn()?;
        let process_guard = match ProcessGuard::attach(&child) {
            Ok(guard) => guard,
            Err(error) => {
                if let Err(kill_error) = child.kill().await {
                    tracing::warn!(plugin_id = %command.plugin_id, "plugin cleanup failed after sandbox setup error: {kill_error}");
                }
                return Err(error);
            }
        };
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginError::Handshake("plugin stdout was not piped".to_owned()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginError::Handshake("plugin stdin was not piped".to_owned()))?;

        let stderr_task = child.stderr.take().map(|stderr| {
            let plugin_id = command.plugin_id.clone();
            let logs = sinks.logs.clone();
            tokio::spawn(async move {
                read_stderr(stderr, &plugin_id, logs.as_deref()).await;
            })
        });

        let client = Self::connect(
            Box::new(stdout),
            Box::new(stdin),
            services,
            Some(child),
            sinks.progress,
            Some(process_guard),
        )
        .await?;
        *client.stderr_task.lock().await = stderr_task;
        Ok(client)
    }
}

/// Reassembles stderr lines across read boundaries, including a final line
/// without a newline. A broken loader cannot grow the pending buffer forever.
async fn read_stderr(
    mut stderr: impl AsyncRead + Unpin,
    plugin_id: &str,
    logs: Option<&super::super::PluginLogStore>,
) {
    let mut buffer = [0_u8; STDERR_CHUNK_SIZE];
    let mut pending: Vec<u8> = Vec::new();
    let emit = |bytes: &[u8]| {
        let message = String::from_utf8_lossy(bytes);
        let message = message.trim_end_matches(['\r', '\n']);
        if message.is_empty() {
            return;
        }
        tracing::info!(target: "plugin", %plugin_id, "{message}");
        if let Some(logs) = logs {
            logs.record_stderr(plugin_id, message);
        }
    };
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => {
                emit(&pending);
                break;
            }
            Ok(length) => {
                pending.extend_from_slice(&buffer[..length]);
                while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = pending.drain(..=index).collect();
                    emit(&line);
                }
                // A plugin that never writes a newline must not
                // grow the buffer without bound.
                if pending.len() > MAX_STDERR_LINE {
                    emit(&pending);
                    pending.clear();
                }
            }
            Err(error) => {
                tracing::debug!(%plugin_id, "plugin stderr stopped: {error}");
                break;
            }
        }
    }
}

pub(super) fn apply_restricted_environment(
    command: &mut Command,
    permissions: &HashSet<PluginPermission>,
) {
    const ALLOWED_ENVIRONMENT: &[&str] = &[
        "HOME",
        "LANG",
        "LC_ALL",
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "WINDIR",
    ];
    let inherited = ALLOWED_ENVIRONMENT
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (*name, value)))
        .collect::<Vec<_>>();
    command.env_clear();
    command.envs(inherited);
    command.env(
        "LOVELYCARVE_NETWORK_PERMISSION",
        if permissions.contains(&PluginPermission::Network) {
            "granted"
        } else {
            "denied"
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::extensions::plugin::PluginLogStore;
    use tokio::io::AsyncWriteExt as _;

    #[tokio::test]
    async fn fragmented_stderr_preserves_utf8_levels_and_the_final_line() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = PluginLogStore::open(directory.path()).expect("log store");
        let (mut writer, reader) = tokio::io::duplex(1);
        let write = async move {
            writer
                .write_all("WARN 日志\r\nINFO done\nloader note".as_bytes())
                .await
                .expect("stderr");
            writer.shutdown().await.expect("close stderr");
        };
        tokio::join!(write, read_stderr(reader, "org.test.stderr", Some(&store)));
        store.shutdown();
        let content = std::fs::read_to_string(store.file_path("org.test.stderr")).expect("log");
        assert_eq!(content.lines().count(), 3);
        assert!(content.contains(" WARN 日志\n"));
        assert!(content.contains(" INFO done\n"));
        assert!(content.contains(" HOST loader note\n"));
    }
}
