//! 日志（#254）：写到 `~/.claude/makit/logs/makit.<日期>.log`，按天滚动、留 7 天。
//!
//! 为什么要有：打包成 .app 后 stderr 没地方看，出问题（hook 装不上、状态文件读不出、PTY 起不来、闪退）时
//! 用户和我们都没有任何线索。公开发布后用户会把日志贴到 issue 里，所以**写进文件之前一律把家目录换成 `~`**，
//! 且只记我们自己的事件，不记对话内容和终端输出。
//!
//! 结构：纯逻辑（`redact`、`diagnostics`、`format_panic`、`log_dir_in`）都有测试；`init` 装 tracing 订阅者 + panic 钩子。
//! 库代码一律用 `log::{info,warn,error}!`，tracing-subscriber 的 `tracing-log` 特性把它们接到同一个订阅者上（GPUI 等第三方的 `log` 输出也一并进来）。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use chrono::{Local, NaiveDate};

/// 日志目录 `<home>/.claude/makit/logs`（和 perf.log 同一个数据目录，搬家时跟着 #249 走）
pub fn log_dir_in(home: &Path) -> PathBuf {
    home.join(".claude").join("makit").join("logs")
}

pub fn log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| log_dir_in(&h))
}

/// 把文本里的家目录换成 `~`，其余 `/Users/<名字>` 换成 `/Users/<user>`（别的用户的路径也不该泄露）。
/// 家目录只在**路径边界**上匹配：`/Users/me` 不能把 `/Users/me2/x` 吃掉一截
pub fn redact(text: &str, home: &str) -> String {
    use regex::Regex;
    let home = home.trim_end_matches('/');
    let mut out = text.to_string();
    if !home.is_empty() {
        // 家目录后面必须是路径边界（斜杠 / 结尾 / 不属于文件名的字符），否则 /Users/me 会吃掉 /Users/me2 的前半截
        let re = Regex::new(&format!(r"{}(/|$|[^A-Za-z0-9._-])", regex::escape(home))).expect("家目录转义后一定是合法正则");
        out = re.replace_all(&out, "~$1").into_owned();
    }
    // 剩下的 /Users/<名字> 是别的用户的路径，也不留名字
    let others = Regex::new(r#"/Users/[^/\s"'`:;,)\]]+"#).expect("固定正则");
    others.replace_all(&out, "/Users/<user>").into_owned()
}

/// 往 `inner` 写之前先 `redact` 的写入器；tracing 的 fmt 层每条事件整行 `write_all` 一次，所以按行脱敏够用
pub struct RedactWriter<W: Write> {
    pub inner: W,
    pub home: String,
}

impl<W: Write> Write for RedactWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = redact(&String::from_utf8_lossy(buf), &self.home);
        self.inner.write_all(line.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// 套在任意 `MakeWriter` 外面，让每个写入器都带脱敏
pub struct RedactMakeWriter<M> {
    pub inner: M,
    pub home: String,
}

impl<'a, M: tracing_subscriber::fmt::MakeWriter<'a>> tracing_subscriber::fmt::MakeWriter<'a> for RedactMakeWriter<M> {
    type Writer = RedactWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactWriter { inner: self.inner.make_writer(), home: self.home.clone() }
    }
}

/// 默认过滤：我们自己的事件记 info 以上，GPUI 等第三方只记 warn 以上；`MAKIT_LOG=debug` 之类可以覆盖
const DEFAULT_FILTER: &str = "info,gpui=warn";
/// 保留几天的日志文件
const KEEP_DAYS: usize = 7;

fn daily_file_name(date: NaiveDate) -> String {
    format!("makit.{}.log", date.format("%Y-%m-%d"))
}

/// 要删掉的旧日志：只看 `makit.<日期>.log`，按日期（文件名字典序即日期序）保留最新的 `keep` 个
fn expired_files(names: &[String], keep: usize) -> Vec<String> {
    let mut logs: Vec<&String> = names.iter().filter(|n| n.starts_with("makit.") && n.ends_with(".log")).collect();
    logs.sort();
    let cut = logs.len().saturating_sub(keep);
    logs[..cut].iter().map(|s| s.to_string()).collect()
}

type DayFile = Option<(NaiveDate, File)>;

/// 按**本地**日期切文件的写入器：跨过本地午夜就换到新文件，并删掉超出保留天数的旧文件。
/// 不用 tracing-appender 的 `DAILY`，它按 UTC 切，东八区 0–8 点的日志会落进前一天的文件
pub struct LocalDailyFiles {
    dir: PathBuf,
    keep: usize,
    current: Mutex<DayFile>,
}

impl LocalDailyFiles {
    pub fn new(dir: PathBuf, keep: usize) -> Self {
        Self { dir, keep, current: Mutex::new(None) }
    }

    fn writer_for(&self, today: NaiveDate) -> DailyWriter<'_> {
        let mut guard = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|(d, _)| *d) != Some(today) {
            *guard = OpenOptions::new().create(true).append(true).open(self.dir.join(daily_file_name(today))).ok().map(|f| (today, f));
            self.prune();
        }
        DailyWriter(guard)
    }

    fn prune(&self) {
        let names: Vec<String> = std::fs::read_dir(&self.dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        for old in expired_files(&names, self.keep) {
            let _ = std::fs::remove_file(self.dir.join(old));
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LocalDailyFiles {
    type Writer = DailyWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        self.writer_for(Local::now().date_naive())
    }
}

pub struct DailyWriter<'a>(MutexGuard<'a, DayFile>);

impl Write for DailyWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.as_mut() {
            Some((_, f)) => f.write_all(buf).map(|_| buf.len()),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.as_mut() {
            Some((_, f)) => f.flush(),
            None => Ok(()),
        }
    }
}

fn macos_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

/// 当前系统的一行描述，诊断信息和启动日志共用
pub fn os_description() -> String {
    format!("{} {}", std::env::consts::OS, macos_version())
}

/// 装日志：文件（按天滚动、留 7 天、脱敏）+ stderr（开发时照旧能看），`log::` 的输出接过来，panic 写进日志，记一行启动信息。
/// 日志目录建不出来 / 订阅者已装过都不是致命问题：只是没有文件日志，程序照常跑
pub fn init() {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
    let filter = EnvFilter::try_from_env("MAKIT_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let timer = fmt::time::ChronoLocal::rfc_3339();
    let file_layer = log_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let files = LocalDailyFiles::new(dir, KEEP_DAYS);
        Some(fmt::layer().with_ansi(false).with_timer(timer.clone()).with_writer(RedactMakeWriter { inner: files, home: home.clone() }))
    });
    let stderr_layer = fmt::layer().with_ansi(false).with_timer(timer).with_writer(std::io::stderr);
    let _ = tracing_subscriber::registry().with(filter).with(file_layer).with(stderr_layer).try_init();

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "（不是字符串的 panic 内容）".into());
        let location = info.location().map(|l| (l.file(), l.line()));
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        log::error!("{}", format_panic(&message, location, &backtrace));
        previous(info);
    }));

    log::info!(
        "makit {} 启动：{}（{}），日志目录 {}",
        env!("CARGO_PKG_VERSION"),
        os_description(),
        std::env::consts::ARCH,
        log_dir().map(|d| d.display().to_string()).unwrap_or_else(|| "（建不出来）".into())
    );
}

/// 最新那个日志文件的最后 `n` 行（诊断信息用）；没有日志返回空
pub fn recent_lines(n: usize) -> Vec<String> {
    let Some(dir) = log_dir() else { return vec![] };
    let mut files: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect();
    files.sort();
    let Some(last) = files.pop() else { return vec![] };
    let text = std::fs::read_to_string(last).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(String::from).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

/// 「复制诊断信息」要放进剪贴板的整段文字
pub fn diagnostics_text() -> String {
    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = log_dir().map(|d| d.display().to_string()).unwrap_or_default();
    diagnostics(env!("CARGO_PKG_VERSION"), &os_description(), std::env::consts::ARCH, &dir, &recent_lines(50), &home)
}

/// panic 记成一行日志：消息 + 位置 + 调用栈（调用栈缩进在后面几行，方便人读）
pub fn format_panic(message: &str, location: Option<(&str, u32)>, backtrace: &str) -> String {
    let at = match location {
        Some((file, line)) => format!("{file}:{line}"),
        None => "未知位置".to_string(),
    };
    let mut s = format!("panic：{message}（{at}）");
    for l in backtrace.lines() {
        s.push_str("\n    ");
        s.push_str(l);
    }
    s
}

/// 「复制诊断信息」的内容：版本、系统、日志目录，加最近几行日志。不含对话和终端输出
pub fn diagnostics(version: &str, os: &str, arch: &str, log_dir: &str, recent: &[String], home: &str) -> String {
    let mut s = format!("makit {version}\n系统：{os}（{arch}）\n日志目录：{log_dir}\n--- 最近日志 ---\n");
    for l in recent {
        s.push_str(l);
        s.push('\n');
    }
    // 整体再脱敏一遍：不管上面哪段拼进了路径，输出里都不会有家目录
    redact(&s, home)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 为什么要测：脱敏漏了，用户把日志贴进公开 issue 就带上了系统用户名（`/Users/<名字>`）。
    /// 边界错了，要么漏（`/Users/me/` 没换），要么误伤（`/Users/me2` 被换成 `~2`）
    #[test]
    fn home_dir_becomes_tilde() {
        assert_eq!(redact("读 /Users/me/.claude/x.json 失败", "/Users/me"), "读 ~/.claude/x.json 失败");
        assert_eq!(redact("cwd=/Users/me", "/Users/me"), "cwd=~", "结尾也算边界");
        assert_eq!(redact("a=/Users/me/p b=/Users/me/q", "/Users/me"), "a=~/p b=~/q", "每处都换");
    }

    #[test]
    fn similar_prefix_is_not_cut_in_half() {
        assert_eq!(redact("/Users/me2/x", "/Users/me"), "/Users/<user>/x", "me2 不是家目录，但仍是别人的用户名路径");
    }

    #[test]
    fn other_users_paths_are_masked_too() {
        assert_eq!(redact("scp /Users/alice/a /Users/bob/b", "/Users/me"), "scp /Users/<user>/a /Users/<user>/b");
    }

    #[test]
    fn empty_home_and_trailing_slash_are_handled() {
        assert_eq!(redact("/tmp/x", ""), "/tmp/x", "家目录未知：不动");
        assert_eq!(redact("/Users/me/a", "/Users/me/"), "~/a", "家目录带结尾斜杠也行");
    }

    #[test]
    fn writer_redacts_every_write_and_reports_the_original_length() {
        let mut w = RedactWriter { inner: Vec::new(), home: "/Users/me".into() };
        let line = b"open /Users/me/.claude/makit failed\n";
        assert_eq!(w.write(line).unwrap(), line.len(), "返回原始长度，调用方才不会重试");
        assert_eq!(String::from_utf8(w.inner).unwrap(), "open ~/.claude/makit failed\n");
    }

    #[test]
    fn log_dir_is_under_the_makit_data_dir() {
        assert_eq!(log_dir_in(Path::new("/h")), PathBuf::from("/h/.claude/makit/logs"));
    }

    #[test]
    fn panic_line_has_message_location_and_backtrace() {
        let s = format_panic("boom", Some(("src/a.rs", 12)), "0: foo\n1: bar");
        assert!(s.contains("panic") && s.contains("boom") && s.contains("src/a.rs:12"), "{s}");
        assert!(s.contains("0: foo") && s.contains("1: bar"));
        assert!(format_panic("x", None, "").contains("未知位置"));
    }

    #[test]
    fn diagnostics_has_environment_and_no_home_path() {
        let recent = vec!["INFO 启动".to_string(), "WARN 读 /Users/me/.claude/x 失败".to_string()];
        let s = diagnostics("0.1.0", "macos 26.0", "x86_64", "/Users/me/.claude/makit/logs", &recent, "/Users/me");
        assert!(s.contains("makit 0.1.0") && s.contains("macos 26.0") && s.contains("x86_64"), "{s}");
        assert!(s.contains("~/.claude/makit/logs"), "日志目录脱敏：{s}");
        assert!(s.contains("INFO 启动") && s.contains("读 ~/.claude/x 失败"), "最近日志也脱敏：{s}");
        assert!(!s.contains("/Users/me"), "任何地方都不能出现家目录：{s}");
    }

    /// 为什么要测：按 UTC 切日，东八区每天 0–8 点的日志会写进「昨天」的文件
    #[test]
    fn daily_file_name_uses_the_given_local_date() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        assert_eq!(daily_file_name(d), "makit.2026-10-04.log");
    }

    #[test]
    fn expired_files_keeps_the_newest_days_and_ignores_other_files() {
        let names: Vec<String> = ["makit.2026-10-02.log", "perf.log", "makit.2026-10-04.log", "makit.2026-10-01.log", "makit.2026-10-03.log"].iter().map(|s| s.to_string()).collect();
        assert_eq!(expired_files(&names, 2), vec!["makit.2026-10-01.log", "makit.2026-10-02.log"]);
        assert!(expired_files(&names, 10).is_empty());
    }

    #[test]
    fn writer_switches_file_when_the_local_day_changes() {
        let dir = std::env::temp_dir().join(format!("makit-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files = LocalDailyFiles::new(dir.clone(), 7);
        let day1 = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let day2 = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        files.writer_for(day1).write_all(b"first\n").unwrap();
        files.writer_for(day2).write_all(b"second\n").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("makit.2026-10-03.log")).unwrap(), "first\n");
        assert_eq!(std::fs::read_to_string(dir.join("makit.2026-10-04.log")).unwrap(), "second\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
