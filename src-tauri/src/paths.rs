//! 路径类小工具：数据目录、打开路径、批量判存在、工具 logo。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub fn projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

// 在 macOS 上打开本地路径：默认在 Finder 中显示（reveal），文件用默认 app 打开
// reveal=true → open -R（Finder 高亮），false → open（用默认 app 打开）
#[tauri::command]
pub async fn open_path(path: String, reveal: bool) -> Result<(), String> {
    // 展开 ~ 为 $HOME
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

/// 一批路径各自存不存在（#214）。终端里的裸目录名、不带斜杠结尾的相对目录，字面上和普通单词分不开，
/// 由这里查磁盘说了算：存在才画成链接。`~` 按 home 展开；相对路径由前端先按终端当前目录拼好。
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

#[tauri::command(async)]
pub fn paths_exist(paths: Vec<String>) -> Vec<bool> {
    // 一次最多查 200 个：悬停一行最多几十个词，超了说明是异常输入，不陪它扫盘
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

/// 获取 AI 工具 logo（base64 data URL）：本地缓存优先，不存在则下载
#[tauri::command]
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
