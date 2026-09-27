use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

fn socket_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("hook.sock"))
}

pub fn start(handle: AppHandle) {
    let Some(path) = socket_path() else { return };
    tauri::async_runtime::spawn(async move {
        run(handle, path).await;
    });
}

async fn run(handle: AppHandle, path: PathBuf) {
    use tokio::net::UnixListener;

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // remove stale socket from previous run
    let _ = std::fs::remove_file(&path);

    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[hook_server] bind error: {e}");
            return;
        }
    };

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let h = handle.clone();
                tauri::async_runtime::spawn(async move {
                    handle_conn(stream, h).await;
                });
            }
            Err(e) => eprintln!("[hook_server] accept error: {e}"),
        }
    }
}

async fn handle_conn(stream: tokio::net::UnixStream, handle: AppHandle) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf = BufReader::new(reader);
    let mut line = String::new();

    // read one newline-delimited JSON payload
    if buf.read_line(&mut line).await.is_ok() && !line.trim().is_empty() {
        let _ = handle.emit("claude-hook", line.trim().to_string());
    }

    // respond {} so Claude Code sees a successful hook exit
    let _ = writer.write_all(b"{}\n").await;
}

use std::fs;

/// 创建 hook 脚本并注入 ~/.claude/settings.json 的 Notification hook
#[tauri::command(async)]
pub fn install_claude_hook() -> Result<String, String> {
    let home = dirs::home_dir().ok_or("无法定位 home 目录")?;

    // 1. write hook script
    let hooks_dir = home.join(".claude").join("hooks");
    fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;

    let hook_script = hooks_dir.join("makit-hook.sh");
    let sock_path = home.join(".claude").join("makit").join("hook.sock");
    let script = format!(
        "#!/bin/sh\nSOCK=\"{}\"\nif [ -S \"$SOCK\" ]; then\n    cat | nc -U \"$SOCK\" 2>/dev/null\nelse\n    echo '{{}}'\nfi\n",
        sock_path.display()
    );
    fs::write(&hook_script, &script).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_script).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_script, perms).map_err(|e| e.to_string())?;
    }

    // 2. update ~/.claude/settings.json
    let settings_path = home.join(".claude").join("settings.json");
    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // check if already installed
    let already = settings
        .pointer("/hooks/Notification")
        .and_then(|n| n.as_array())
        .map(|groups| {
            groups.iter().any(|g| {
                g.get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|cmds| {
                        cmds.iter().any(|c| {
                            c.get("command")
                                .and_then(|v| v.as_str())
                                .map(|s| s.contains("makit-hook.sh"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    if already {
        return Ok("already_installed".into());
    }

    let hook_cmd = format!("{} 2>/dev/null || echo '{{}}'", hook_script.display());
    let new_group = serde_json::json!({
        "hooks": [{"type": "command", "command": hook_cmd}]
    });

    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json 格式错误")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let notif = hooks
        .as_object_mut()
        .ok_or("hooks 格式错误")?
        .entry("Notification")
        .or_insert_with(|| serde_json::json!([]));

    notif
        .as_array_mut()
        .ok_or("Notification 不是数组")?
        .push(new_group);

    let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    fs::write(&settings_path, out).map_err(|e| e.to_string())?;

    Ok("installed".into())
}
