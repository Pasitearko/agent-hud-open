//! Settings → Agents → Agent plugins: Agent HUD's Claude Code plugin, which
//! lets the HUD answer Claude's questions in the session that asked. It is
//! installed and removed only when the user asks there, with Claude Code's own
//! `plugin install`; the channel it shares with the app and the HUD's answers
//! live in `agent_hud_runtime::claude_plugin`.
use agent_hud_runtime::claude_plugin::{self as plugin, ClaudeChannel, ClaudeReplies};
use agent_hud_runtime::replies::SessionReplies;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tauri::Manager;

/// Claude Code fetches nothing to install a local plugin, but may take a while.
const COMMAND_TIME: Duration = Duration::from_secs(60);

pub struct ClaudePlugin {
    channel: ClaudeChannel,
    folder: PathBuf,
    owner: Mutex<()>,
}

impl ClaudePlugin {
    /// The plugin's channel and files: where a product build keeps them, else
    /// in the app's data folder.
    pub(crate) fn new(paths: Option<(PathBuf, PathBuf)>) -> Result<Self, String> {
        let (channel, folder) = match paths {
            Some(paths) => paths,
            None => {
                let data = agent_hud_runtime::local_tools_directory()?;
                (data.join("claude-channel"), data.join("claude-plugin"))
            }
        };
        Ok(Self {
            channel: ClaudeChannel::new(channel),
            folder,
            owner: Mutex::new(()),
        })
    }

    /// The HUD's way to answer Claude's questions in a session running the plugin.
    pub(crate) fn replies(&self) -> Arc<dyn SessionReplies> {
        let claude_config = super::mcp_clients::home()
            .map(|home| plugin::claude_config_directory(&home))
            .unwrap_or_default();
        Arc::new(ClaudeReplies::new(self.channel.clone(), claude_config))
    }
}

/// Whether Claude Code is on this Mac, whether it lists the plugin, and how
/// many sessions run it now.
fn status(channel: &ClaudeChannel, executable: Option<&Path>, claude_config: &Path) -> Value {
    json!({
        "found": executable.is_some(),
        "installed": plugin::installed(claude_config),
        "sessions": channel.loaded(SystemTime::now()).len(),
    })
}

/// Writes the plugin's files and has Claude Code install them, unless it lists the plugin already.
fn install(
    channel: &ClaudeChannel,
    folder: &Path,
    executable: Option<&Path>,
    claude_config: &Path,
    home: &Path,
) -> Result<(), String> {
    let executable = executable.ok_or("Claude Code's command line is not installed")?;
    channel.prepare_plugin(folder)?;
    if plugin::installed(claude_config) {
        return Ok(());
    }
    super::mcp_clients::run_client(
        executable,
        &plugin::install_arguments(folder),
        home,
        COMMAND_TIME,
    )
}

/// Forgets every check-in and word left, then has Claude Code remove the
/// plugin and its marketplace.
fn remove(
    channel: &ClaudeChannel,
    executable: Option<&Path>,
    claude_config: &Path,
    home: &Path,
) -> Result<(), String> {
    channel.clear()?;
    if !plugin::installed(claude_config) {
        return Ok(());
    }
    let executable = executable.ok_or("Claude Code's command line is not installed")?;
    let [uninstall, marketplace] = plugin::remove_arguments();
    super::mcp_clients::run_client(executable, &uninstall, home, COMMAND_TIME)?;
    let _ = super::mcp_clients::run_client(executable, &marketplace, home, COMMAND_TIME);
    Ok(())
}

#[tauri::command]
pub async fn native_claude_plugin(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    action: String,
) -> Result<Value, String> {
    if window.label() != "main" {
        return Err("The Claude Code plugin is set up in Settings".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ClaudePlugin>();
        let _owner = state
            .owner
            .lock()
            .map_err(|_| "The plugin's owner is unavailable")?;
        let home = super::mcp_clients::home()?;
        let executable = super::mcp_clients::executable("claudeCode", &home);
        let config = plugin::claude_config_directory(&home);
        match action.as_str() {
            "status" => {}
            "install" => install(
                &state.channel,
                &state.folder,
                executable.as_deref(),
                &config,
                &home,
            )?,
            "remove" => remove(&state.channel, executable.as_deref(), &config, &home)?,
            _ => return Err("Unknown plugin action".into()),
        }
        Ok(status(&state.channel, executable.as_deref(), &config))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;

    /// Stands in for Claude Code: `plugin install` and `plugin uninstall`
    /// change its record of installed plugins as the real one does, and every
    /// call is logged.
    fn fake_claude(root: &Path, config: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let record = config.join("plugins/installed_plugins.json");
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        let claude = root.join("claude");
        fs::write(
            &claude,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{log}'\ncase \"$1 $2\" in\n\"plugin install\") printf '%s' '{on}' > '{record}' ;;\n\"plugin uninstall\") printf '%s' '{off}' > '{record}' ;;\nesac\n",
                log = root.join("calls").display(),
                record = record.display(),
                on = r#"{"plugins":{"agent-hud@agent-hud":[]}}"#,
                off = r#"{"plugins":{}}"#,
            ),
        )
        .unwrap();
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
        claude
    }

    #[test]
    fn install_and_remove_go_through_claude_code_and_status_follows() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let config = root.join("claude-config");
        let channel = ClaudeChannel::new(root.join("channel"));
        let folder = root.join("plugin");
        let claude = fake_claude(root, &config);

        assert_eq!(
            status(&channel, None, &config),
            json!({"found":false,"installed":false,"sessions":0})
        );
        assert!(
            install(&channel, &folder, None, &config, root).is_err(),
            "no Claude Code, nothing to install into"
        );

        install(&channel, &folder, Some(&claude), &config, root).unwrap();
        assert!(folder.join("hooks/register.ts").is_file());
        // Installed already: the files are refreshed, Claude Code is not asked again.
        install(&channel, &folder, Some(&claude), &config, root).unwrap();
        let calls = fs::read_to_string(root.join("calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("plugin install agent-hud --marketplace"))
                .count(),
            1
        );

        let session = "44444444-4444-4444-8444-444444444444";
        fs::write(
            channel.directory().join(format!("sessions/{session}.json")),
            br#"{"at":1}"#,
        )
        .unwrap();
        assert_eq!(
            status(&channel, Some(&claude), &config),
            json!({"found":true,"installed":true,"sessions":1})
        );

        remove(&channel, Some(&claude), &config, root).unwrap();
        assert_eq!(
            status(&channel, Some(&claude), &config),
            json!({"found":true,"installed":false,"sessions":0})
        );
        assert!(!channel.directory().join("sessions").exists());
        let calls = fs::read_to_string(root.join("calls")).unwrap();
        assert!(calls.contains("plugin uninstall agent-hud@agent-hud"));
        assert!(calls.contains("plugin marketplace remove agent-hud"));
        // Removing what is not there asks nothing of Claude Code.
        remove(&channel, None, &config, root).unwrap();
    }
}
