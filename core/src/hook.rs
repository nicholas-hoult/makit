//! Claude Code Notification hook: hook script installation + a local unix socket server.
//!
//! Protocol: `~/.claude/makit/hook.sock`. Each connection reads only **one line** of JSON and hands it to the callback as-is;
//! then it writes back `{}\n`, so Claude Code considers the hook to have run successfully.
//!
//! No UI framework / async runtime dependency: it brings its own accept thread, with a short-lived thread per connection
//! (a hook sends one line at a time and rarely, so a runtime is not worth pulling in).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn socket_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("hook.sock"))
}

/// Start the hook server on `~/.claude/makit/hook.sock`. `on_line` is called once for each received line (trimmed, non-empty).
/// `on_line` is invoked on the connection thread and may run concurrently, so it must be `Sync`.
pub fn start(on_line: impl Fn(String) + Send + Sync + 'static) {
    let Some(path) = socket_path() else { return };
    if let Err(e) = start_at(&path, on_line) {
        log::error!(target: "hook_server", "bind error: {e}");
    }
}

/// Testable version of `start`: the socket path can be injected. A bind failure returns an error synchronously; on success the accept loop runs on a background thread.
pub fn start_at(path: &Path, on_line: impl Fn(String) + Send + Sync + 'static) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // A socket file left over from the last run: bind fails unless it is removed
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    let on_line = Arc::new(on_line);
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let cb = on_line.clone();
                    std::thread::spawn(move || handle_conn(stream, &*cb));
                }
                Err(e) => log::warn!(target: "hook_server", "accept error: {e}"),
            }
        }
    });
    Ok(())
}

fn handle_conn(stream: UnixStream, on_line: &(dyn Fn(String) + Send + Sync)) {
    let mut line = String::new();
    // Read only one newline-terminated line of JSON
    if let Ok(reader) = stream.try_clone() {
        let mut buf = BufReader::new(reader);
        if buf.read_line(&mut line).is_ok() && !line.trim().is_empty() {
            on_line(line.trim().to_string());
        }
    }
    // Reply with {} so Claude Code considers the hook to have exited successfully
    let mut w = &stream;
    let _ = w.write_all(b"{}\n");
}

/// Claude Code hook events makit uses: `Notification` (awaiting approval / an answer), `Stop` (task finished, carrying Claude's own words as the banner body),
/// `UserPromptSubmit` / `SessionEnd` (clear unread). With only Notification installed, "task finished" has no body text and no banner can be shown
pub const HOOK_EVENTS: [&str; 4] = ["Notification", "Stop", "UserPromptSubmit", "SessionEnd"];

/// Create the hook script and register `HOOK_EVENTS` in ~/.claude/settings.json
pub fn install_claude_hook() -> Result<String, String> {
    let home = dirs::home_dir().ok_or("无法定位 home 目录")?;
    install_claude_hook_in(&home)
}

/// Returns `"installed"` (first install) / `"updated"` (missing events added) / `"already_installed"`.
/// If settings.json exists but cannot be parsed, it is **left untouched** and an error is returned (it used to be replaced with an empty config and written back, wiping the user's whole Claude configuration)
pub fn install_claude_hook_in(home: &Path) -> Result<String, String> {
    // 1. hook script
    let hooks_dir = home.join(".claude").join("hooks");
    fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;
    let hook_script = hooks_dir.join("makit-hook.sh");
    let sock_path = home.join(".claude").join("makit").join("hook.sock");
    let script = format!(
        "#!/bin/sh\nSOCK=\"{}\"\nif [ -S \"$SOCK\" ]; then\n    cat | nc -U \"$SOCK\" 2>/dev/null\nelse\n    echo '{{}}'\nfi\n",
        sock_path.display()
    );
    fs::write(&hook_script, &script).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_script).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_script, perms).map_err(|e| e.to_string())?;
    }

    // 2. settings.json: if missing, treat as empty config; if it exists but cannot be parsed -> leave it untouched, report an error
    let settings_path = home.join(".claude").join("settings.json");
    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| {
            format!("~/.claude/settings.json 不是合法的 JSON（{e}），为避免覆盖你的配置没有改动；请先修好它再装")
        })?
    } else {
        serde_json::json!({})
    };

    let has_makit = |groups: &serde_json::Value| {
        groups.as_array().is_some_and(|gs| {
            gs.iter().any(|g| {
                g.get("hooks").and_then(|h| h.as_array()).is_some_and(|cmds| {
                    cmds.iter().any(|c| c.get("command").and_then(|v| v.as_str()).is_some_and(|s| s.contains("makit-hook.sh")))
                })
            })
        })
    };
    let hook_cmd = format!("{} 2>/dev/null || echo '{{}}'", hook_script.display());
    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json 格式错误")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("hooks 格式错误")?;
    let had_any = HOOK_EVENTS.iter().any(|e| hooks.get(*e).is_some_and(&has_makit));
    let mut added = 0;
    for ev in HOOK_EVENTS {
        let groups = hooks.entry(ev).or_insert_with(|| serde_json::json!([]));
        if has_makit(groups) {
            continue;
        }
        groups
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{ev} 不是数组"))?
            .push(serde_json::json!({"hooks": [{"type": "command", "command": hook_cmd}]}));
        added += 1;
    }
    if added == 0 {
        return Ok("already_installed".into());
    }
    let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    fs::write(&settings_path, out).map_err(|e| e.to_string())?;
    Ok(if had_any { "updated" } else { "installed" }.into())
}

/// After the callback-style refactor (#226): the hook server no longer depends on tauri's async runtime and starts its own thread.
/// What goes wrong on screen if this breaks: no notification when claude awaits approval / an answer (the line was not handed over), or claude's hook
/// hangs / reports a hook failure (`{}` was not sent back).
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn delivers_one_line_and_replies_empty_object() {
        // unix socket paths have a length limit (104 bytes), use a short directory
        let dir = PathBuf::from(format!("/tmp/mk-hook-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let sock = dir.join("h.sock");
        // A leftover old socket file must not block bind
        fs::create_dir_all(&dir).unwrap();
        fs::write(&sock, "stale").unwrap();

        let (tx, rx) = mpsc::channel::<String>();
        let tx = std::sync::Mutex::new(tx);
        start_at(&sock, move |l| {
            let _ = tx.lock().unwrap().send(l);
        })
        .expect("bind 失败");

        for payload in ["{\"hook_event_name\":\"Notification\",\"session_id\":\"a\"}", "{\"n\":2}"] {
            let mut s = UnixStream::connect(&sock).unwrap();
            s.write_all(format!("  {payload}\n").as_bytes()).unwrap();
            let mut reply = String::new();
            s.read_to_string(&mut reply).unwrap();
            assert_eq!(reply, "{}\n", "必须回 {{}}，否则 claude 认为 hook 失败");
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), payload, "去掉首尾空白后原样交出");
        }

        // empty line: still reply {}, but do not call back
        let mut s = UnixStream::connect(&sock).unwrap();
        s.write_all(b"\n").unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "{}\n");
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "空行不该回调");
        let _ = fs::remove_dir_all(&dir);
    }

    // ---- install_claude_hook_in ----

    fn temp_home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("makit-hook-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn settings(home: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(home.join(".claude/settings.json")).unwrap()).unwrap()
    }
    fn events_with_makit(v: &serde_json::Value) -> Vec<String> {
        let mut out: Vec<String> = v["hooks"]
            .as_object()
            .map(|h| h.iter().filter(|(_, g)| g.to_string().contains("makit-hook.sh")).map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    #[test]
    fn fresh_install_registers_every_event_and_writes_an_executable_script() {
        let home = temp_home("fresh");
        assert_eq!(install_claude_hook_in(&home).unwrap(), "installed");
        let mut want: Vec<String> = HOOK_EVENTS.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(events_with_makit(&settings(&home)), want, "四个事件都要注册，缺 Stop 则「任务完成」拿不到正文");
        let script = home.join(".claude/hooks/makit-hook.sh");
        let body = fs::read_to_string(&script).unwrap();
        assert!(body.contains(&home.join(".claude/makit/hook.sock").display().to_string()), "脚本指向本机的 socket");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&script).unwrap().permissions().mode() & 0o777, 0o755);
    }

    #[test]
    fn install_keeps_the_users_other_settings_and_hooks() {
        let home = temp_home("keep");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude/settings.json"),
            r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]}],"PreToolUse":[{"hooks":[{"type":"command","command":"lint"}]}]}}"#,
        )
        .unwrap();
        install_claude_hook_in(&home).unwrap();
        let v = settings(&home);
        assert_eq!(v["model"], "opus");
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "lint");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "用户自己的 Stop hook 还在，我们的追加在后面");
        assert_eq!(stop[0]["hooks"][0]["command"], "echo mine");
    }

    #[test]
    fn second_install_changes_nothing() {
        let home = temp_home("twice");
        install_claude_hook_in(&home).unwrap();
        let before = fs::read_to_string(home.join(".claude/settings.json")).unwrap();
        assert_eq!(install_claude_hook_in(&home).unwrap(), "already_installed");
        assert_eq!(fs::read_to_string(home.join(".claude/settings.json")).unwrap(), before, "重复装不重复写");
    }

    #[test]
    fn an_old_install_with_only_notification_gets_the_missing_events() {
        let home = temp_home("upgrade");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude/settings.json"),
            r#"{"hooks":{"Notification":[{"hooks":[{"type":"command","command":"/x/.claude/hooks/makit-hook.sh 2>/dev/null || echo '{}'"}]}]}}"#,
        )
        .unwrap();
        assert_eq!(install_claude_hook_in(&home).unwrap(), "updated");
        let v = settings(&home);
        assert_eq!(v["hooks"]["Notification"].as_array().unwrap().len(), 1, "已有的 Notification 不重复加");
        assert_eq!(events_with_makit(&v).len(), 4);
    }

    #[test]
    fn unparseable_settings_json_is_left_alone() {
        let home = temp_home("corrupt");
        fs::create_dir_all(home.join(".claude")).unwrap();
        let path = home.join(".claude/settings.json");
        let original = "{ \"model\": \"opus\", // 手写的注释\n \"hooks\": {} ";
        fs::write(&path, original).unwrap();
        let err = install_claude_hook_in(&home).unwrap_err();
        assert!(err.contains("settings.json"), "报错要说清是哪个文件：{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), original, "解析不了就原样留着，不能拿空配置覆盖");
    }
}
