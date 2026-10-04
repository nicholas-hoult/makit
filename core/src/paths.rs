//! Path utilities: data directory, opening paths, batch existence checks, tool logos.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub fn projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

// Open a local path on macOS: reveal in Finder by default, open files with the default app.
// reveal=true -> open -R (highlight in Finder), false -> open (default app)
pub fn open_path(path: String, reveal: bool) -> Result<(), String> {
    // Expand ~ to $HOME
    let expanded = if path.starts_with("~/") || path == "~" {
        let home = std::env::var("HOME").map_err(|_| "无法获取 HOME".to_string())?;
        if path == "~" {
            home
        } else {
            format!("{}/{}", home, &path[2..])
        }
    } else {
        path.clone()
    };
    let p = std::path::Path::new(&expanded);
    if !p.exists() {
        return Err(format!("路径不存在: {}", expanded));
    }
    let mut cmd = Command::new("open");
    if reveal {
        cmd.arg("-R");
    }
    cmd.arg(&expanded);
    cmd.spawn().map_err(|e| format!("open 失败: {}", e))?;
    Ok(())
}

/// Whether each path in a batch exists (#214). A bare directory name in a terminal, or a relative directory without a trailing slash,
/// is textually indistinguishable from an ordinary word, so the disk decides: only existing paths become links. `~` is expanded against home; relative paths are joined with the terminal's current directory by the frontend first.
pub fn paths_exist_in(paths: &[String], home: &std::path::Path) -> Vec<bool> {
    paths
        .iter()
        .map(|p| {
            let expanded = if p == "~" {
                home.to_path_buf()
            } else if let Some(rest) = p.strip_prefix("~/") {
                home.join(rest)
            } else {
                std::path::PathBuf::from(p)
            };
            expanded.exists()
        })
        .collect()
}

pub fn paths_exist(paths: Vec<String>) -> Vec<bool> {
    // At most 200 per call: hovering over one line yields a few dozen words at most; more than that is abnormal input, so don't scan the disk for it
    let paths: Vec<String> = paths.into_iter().take(200).collect();
    match dirs::home_dir() {
        Some(home) => paths_exist_in(&paths, &home),
        None => paths.iter().map(|p| std::path::Path::new(p).exists()).collect(),
    }
}

#[cfg(test)]
mod paths_exist_tests {
    use super::paths_exist_in;

    #[test]
    fn checks_absolute_relative_and_tilde() {
        let home = std::env::temp_dir().join(format!("makit-paths-exist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("docs/issue")).unwrap();
        let abs = home.join("docs").to_string_lossy().into_owned();
        let got = paths_exist_in(
            &[abs.clone(), format!("{abs}/issue"), format!("{abs}/nope"), "~/docs".into(), "~/missing".into(), "~".into()],
            &home,
        );
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(got, vec![true, true, false, true, false, true]);
    }
}

/// Fetch an AI tool logo (base64 data URL): local cache first, download if missing
pub async fn get_tool_logo(tool: String) -> Result<String, String> {
    let (url, ext, mime) = match tool.as_str() {
        "claude" => ("https://www.anthropic.com/favicon.ico", "ico", "image/x-icon"),
        "codex"  => ("https://openai.com/favicon.ico", "ico", "image/x-icon"),
        other    => return Err(format!("unknown tool: {}", other)),
    };

    let cache_dir = dirs::home_dir()
        .ok_or("no home dir")?
        .join(".claude").join("makit").join("logos");
    fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

    let local_path = cache_dir.join(format!("{}.{}", tool, ext));

    let bytes: Vec<u8> = if local_path.exists() {
        fs::read(&local_path).map_err(|e| e.to_string())?
    } else {
        let b = reqwest::get(url).await
            .map_err(|e| e.to_string())?
            .bytes().await
            .map_err(|e| e.to_string())?
            .to_vec();
        fs::write(&local_path, &b).map_err(|e| e.to_string())?;
        b
    };

    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, encoded))
}
