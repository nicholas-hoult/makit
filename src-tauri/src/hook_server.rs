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
