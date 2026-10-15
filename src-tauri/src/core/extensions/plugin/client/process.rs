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

        if let Some(stderr) = child.stderr.take() {
            let plugin_id = command.plugin_id.clone();
            let logs = sinks.logs.clone();
            tokio::spawn(async move {
                let mut stderr = stderr;
                let mut buffer = [0_u8; STDERR_CHUNK_SIZE];
                // stderr arrives in arbitrary chunks, so lines are cut at
                // their newline instead of at the read boundary; the loader
                // prefixes every complete line with the level the plugin
                // asked for.
                let mut pending: Vec<u8> = Vec::new();
                let emit = |bytes: &[u8]| {
                    let message = String::from_utf8_lossy(bytes);
                    let message = message.trim_end_matches(['\r', '\n']);
                    if message.is_empty() {
                        return;
                    }
                    tracing::info!(target: "plugin", %plugin_id, "{message}");
                    if let Some(logs) = &logs {
                        logs.record_stderr(&plugin_id, message);
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
            });
        }

        Self::connect(
            Box::new(stdout),
            Box::new(stdin),
            services,
            Some(child),
            sinks.progress,
            Some(process_guard),
        )
        .await
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
