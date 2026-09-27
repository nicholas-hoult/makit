//! 运行状态：`~/.claude/sessions/*.json`（claude 运行时写的 pid / 状态 / 名字），
//! 以及 pty_id → session 的绑定。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::process::get_env_var_of_pid;

#[derive(Clone, Debug)]
pub struct RunningInfo {
    pub name: String,
    pub status: String,
    pub pty_id: String,
    pub waiting_for: String,
    pub pid: u32,
}

pub fn load_running_info() -> HashMap<String, RunningInfo> {
    let mut map = HashMap::new();
    let dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("sessions"),
        None => return map,
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return map,
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let session_id = match v.get("sessionId").and_then(|x| x.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let status = v
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let waiting_for = v
            .get("waitingFor")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        // 检查 claude 进程的环境变量获取 MAKIT_PTY_ID
        let pty_id = if pid > 0 {
            get_env_var_of_pid(pid, "MAKIT_PTY_ID").unwrap_or_default()
        } else {
            String::new()
        };
        map.insert(
            session_id,
            RunningInfo {
                name,
                status,
                waiting_for,
                pid,
                pty_id,
            },
        );
    }
    map
}

// 轻量扫描：只读 running 状态（~/.claude/sessions/*.json），不扫 projects
pub fn list_running_sessions() -> Vec<RunningMeta> {
    let home = match dirs::home_dir() { Some(h) => h, None => return vec![] };
    let sessions_dir = home.join(".claude").join("sessions");
    let mut result = vec![];
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(true, |e| e != "json") { continue; }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    let session_id = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let waiting_for = v.get("waitingFor").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                    // name 必须带上：改名写的就是这个文件，而写它只会触发 `running-changed`。
                    // 少了这个字段，那条路就**结构上**搬不了标题，改名要 ⌘R 才生效（#4 / #8）。
                    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if !session_id.is_empty() {
                        result.push(RunningMeta { session_id, status, waiting_for, pid, name });
                    }
                }
            }
        }
    }
    result
}

#[derive(serde::Serialize)]
pub struct RunningMeta {
    pub session_id: String,
    pub status: String,
    pub waiting_for: String,
    pub pid: u32,
    /// claude 自己写的会话名（`~/.claude/sessions/<pid>.json` 的 `name`）。可能为空。
    /// 和 `RunningInfo.name` 同源；`parse_session` 里它是 display_name 的**最高优先级**。
    pub name: String,
}

#[derive(serde::Serialize)]
pub struct PtyBinding {
    pub pty_id: String,
    pub session_id: String,
    pub short_id: String,
    /// claude 自己写的会话名（`~/.claude/sessions/<pid>.json` 的 `name`）。可能为空。
    pub name: String,
}

/// 把「还没绑上 session 的 tab」和「正在跑的 claude」对上：pty_id → session_id。
///
/// 为什么不复用 `list_sessions`：绑定需要的事实只有 `(pty_id, session_id)`，它在 claude
/// 启动的那一瞬间就写进了 `~/.claude/sessions/<pid>.json`。而 `list_sessions` 扫的是
/// `~/.claude/projects/**/*.jsonl` —— **那个文件要等用户发出第一条消息之后才存在**。
/// 于是「新建 shell、手打 claude」的 tab 在发消息之前根本不在 sessions 数组里，
/// `s.pty_id === t.id` 无从匹配，标题一直停在「新会话」；⌘R 走的还是 `list_sessions`，
/// 所以刷新同样无效；而 resume 出来的 tab 早就有 jsonl，看着"是好的"。
/// 三种表现是同一个根因：绑定挂在了一个比它自己晚出现的数据源上。
///
/// 只对前端传进来的 pty_ids 干活：没有待绑定 tab 时前端不会调这个命令，一次 ps 都不跑。
/// 匹配满了就早退，正常情况（一个新 tab）只查到第一个命中就结束。
pub fn resolve_pty_bindings(pty_ids: Vec<String>) -> Vec<PtyBinding> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    resolve_bindings_in(
        &home.join(".claude").join("sessions"),
        &pty_ids,
        // 「这个 pid 现在归哪个 pty」合成一问：进程已经没了就当没有 marker。
        // sessions/*.json 不保证在 claude 退出时被清掉，先 kill(0) 挡掉死 pid，
        // 省一次 ps。
        &|pid| {
            if !pid_alive(pid) {
                return None;
            }
            get_env_var_of_pid(pid, "MAKIT_PTY_ID")
        },
    )
}

/// `resolve_pty_bindings` 的纯逻辑部分：目录 + 想要的 pty_ids + 「pid → pty_id」查询。
/// 抽出来是为了能测 —— 真实实现要一个活着的、带 MAKIT_PTY_ID 的非 Apple 签名进程，
/// 在单测里造不出来。
pub fn resolve_bindings_in(
    dir: &Path,
    pty_ids: &[String],
    pty_of_pid: &dyn Fn(u32) -> Option<String>,
) -> Vec<PtyBinding> {
    if pty_ids.is_empty() {
        return vec![];
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    let mut result: Vec<PtyBinding> = vec![];
    for entry in entries.flatten() {
        // 每个 tab 只能绑一个 session，配额满了就不必再扫（常见情况是只有一个新 tab）
        if result.len() == pty_ids.len() {
            break;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let v: serde_json::Value = match fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => continue,
        };
        let session_id = match v.get("sessionId").and_then(|x| x.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if pid == 0 {
            continue;
        }
        let pty_id = match pty_of_pid(pid) {
            Some(p) if pty_ids.iter().any(|want| *want == p) => p,
            _ => continue,
        };
        result.push(PtyBinding {
            pty_id,
            short_id: session_id.split('-').next().unwrap_or("").to_string(),
            session_id,
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    result
}

#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    // signal 0：只做存在性/权限检查，不真的发信号
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod pty_binding_tests {
    use std::fs;

    /// 回归 #169 的第二半：「新建 shell 里手敲 claude」的 tab 标题不同步。
    ///
    /// 原来的绑定挂在 `sessions` 数组上（`list_sessions` → 扫
    /// `~/.claude/projects/**/*.jsonl`），而那个 jsonl **要等用户发出第一条消息之后
    /// 才被创建**。于是新起的 claude 在发消息前根本不在 sessions 里，
    /// `s.pty_id === t.id` 无从匹配，tab.sessionId 一直是 null，标题停在「新会话」。
    /// ⌘R 走的还是 `list_sessions`，所以"刷新也没修复"；resume 出来的 tab 早就有
    /// jsonl，所以"恢复的是可以的"——用户给的三句话是同一个根因的三个侧面。
    ///
    /// 真正需要的事实只有 `(pty_id, session_id)`，claude 启动那一刻就写进了
    /// `~/.claude/sessions/<pid>.json`。这里测的就是"只读那个目录也够"。
    ///
    /// 实测证据（修之前）：pid 86431 的 MAKIT_PTY_ID=t_msngclks3eo5、session 32318d5f
    /// 活了近 2 小时，而 localStorage 里 id 为 t_msngclks3eo5 的 tab 仍是
    /// `kind:"new" / label:"新会话" / sessionId:null` —— 因为 32318d5f.jsonl 不存在。
    fn write(dir: &std::path::Path, file: &str, json: &str) {
        fs::write(dir.join(file), json).unwrap();
    }

    #[test]
    fn binds_from_sessions_dir_without_any_jsonl() {
        let dir = std::env::temp_dir().join(format!("makit-bind-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 想绑的那个：pid 4321 → t_wanted
        write(
            &dir,
            "4321.json",
            r#"{"pid":4321,"sessionId":"32318d5f-1111-2222-3333-444455556666","name":"ai-claw-studio-6d"}"#,
        );
        // 别人家的 pty，不能串台
        write(
            &dir,
            "4322.json",
            r#"{"pid":4322,"sessionId":"deadbeef-0000-0000-0000-000000000000","name":"别人"}"#,
        );
        // pid=0（claude 没写 pid）：必须跳过，且不该去查环境变量
        write(
            &dir,
            "4323.json",
            r#"{"pid":0,"sessionId":"aaaaaaaa-0000-0000-0000-000000000000"}"#,
        );
        // 不是 json 后缀 / 坏 json：都不能让整个扫描挂掉
        write(&dir, "notes.txt", "t_wanted");
        write(&dir, "broken.json", "{ this is not json");

        let asked = std::cell::RefCell::new(Vec::<u32>::new());
        let pty_of = |pid: u32| -> Option<String> {
            asked.borrow_mut().push(pid);
            match pid {
                4321 => Some("t_wanted".into()),
                4322 => Some("t_someone_else".into()),
                _ => None,
            }
        };

        let out = super::resolve_bindings_in(&dir, &["t_wanted".to_string()], &pty_of);

        assert_eq!(out.len(), 1, "只该绑上 t_wanted 那一个");
        assert_eq!(out[0].pty_id, "t_wanted");
        assert_eq!(out[0].session_id, "32318d5f-1111-2222-3333-444455556666");
        assert_eq!(out[0].short_id, "32318d5f", "short_id 取第一段 uuid");
        assert_eq!(
            out[0].name, "ai-claw-studio-6d",
            "要把 claude 写的会话名带回去，否则 tab 只能显示 [shortId]"
        );
        assert!(
            !asked.borrow().contains(&0),
            "pid=0 不该触发环境变量查询（每次查询是一个 ps 进程）"
        );

        // pty_ids 为空：一次目录读都不该发生 —— 前端没有待绑定 tab 时的零成本路径
        let never = |_pid: u32| -> Option<String> {
            panic!("pty_ids 为空时不该查任何 pid");
        };
        assert!(super::resolve_bindings_in(&dir, &[], &never).is_empty());

        // 匹配不上就是空，而不是"随便挑一个回去"
        let out = super::resolve_bindings_in(&dir, &["t_nobody".to_string()], &pty_of);
        assert!(out.is_empty(), "没有命中的 pty_id 时必须返回空");

        let _ = fs::remove_dir_all(&dir);
    }
}
