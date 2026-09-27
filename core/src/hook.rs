//! Claude Code 的 Notification hook：hook 脚本安装 + 本机 unix socket 服务。
//!
//! 协议：`~/.claude/makit/hook.sock`。每个连接只读**一行** JSON，原样交给回调；
//! 然后回写 `{}\n`，让 Claude Code 认为 hook 执行成功。
//!
//! 不依赖任何 UI 框架 / async runtime：自带一个接受线程，每个连接一个短命线程
//! （hook 一次一行、频率很低，不值得为它拉一个 runtime）。

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn socket_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("hook.sock"))
}

/// 在 `~/.claude/makit/hook.sock` 上起 hook 服务。收到的每一行（去掉首尾空白、非空）调一次 `on_line`。
/// `on_line` 在连接线程里被调用，可能并发，所以要 `Sync`。
pub fn start(on_line: impl Fn(String) + Send + Sync + 'static) {
    let Some(path) = socket_path() else { return };
    if let Err(e) = start_at(&path, on_line) {
        eprintln!("[hook_server] bind error: {e}");
    }
}

/// `start` 的可测版本：socket 路径可注入。绑定失败同步返回错误；成功后接受循环在后台线程里跑。
pub fn start_at(path: &Path, on_line: impl Fn(String) + Send + Sync + 'static) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // 上次运行残留的 socket 文件：不删就 bind 不上
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
                Err(e) => eprintln!("[hook_server] accept error: {e}"),
            }
        }
    });
    Ok(())
}

fn handle_conn(stream: UnixStream, on_line: &(dyn Fn(String) + Send + Sync)) {
    let mut line = String::new();
    // 只读一行 newline 结尾的 JSON
    if let Ok(reader) = stream.try_clone() {
        let mut buf = BufReader::new(reader);
        if buf.read_line(&mut line).is_ok() && !line.trim().is_empty() {
            on_line(line.trim().to_string());
        }
    }
    // 回 {}，让 Claude Code 认为 hook 成功退出
    let mut w = &stream;
    let _ = w.write_all(b"{}\n");
}

/// 创建 hook 脚本并注入 ~/.claude/settings.json 的 Notification hook
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

/// 回调式改造（#226）：hook 服务不再依赖 tauri 的 async runtime，自己起线程。
/// 错了在 UI 上：claude 等审批 / 等回答时收不到通知（行没交出来），或 claude 的 hook
/// 卡住、报 hook 失败（没回 `{}`）。
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn delivers_one_line_and_replies_empty_object() {
        // unix socket 路径有长度上限（104 字节），用短目录
        let dir = PathBuf::from(format!("/tmp/mk-hook-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let sock = dir.join("h.sock");
        // 残留的旧 socket 文件不能挡住 bind
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

        // 空行：照样回 {}，但不回调
        let mut s = UnixStream::connect(&sock).unwrap();
        s.write_all(b"\n").unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "{}\n");
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "空行不该回调");
        let _ = fs::remove_dir_all(&dir);
    }
}
