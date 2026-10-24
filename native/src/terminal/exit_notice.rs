//! The line written into the terminal when its child process ends (#288).
//!
//! Why a separate leaf module with tests: the handler for the exit event lives in the gpui entity and cannot be unit
//! tested, but the text is the whole user-visible behaviour. Dropping the status (the old `ChildExit(_)`) made
//! "claude crashed" and "claude was quit normally" look identical on screen: both just said "process exited".

use std::process::ExitStatus;

use crate::ts;

/// Text for the dim line shown after the process ends. `None` = the event carried no status (`Event::Exit`).
pub fn exit_notice(status: Option<ExitStatus>) -> String {
    let Some(status) = status else { return ts!("terminal.exited") };
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ts!("terminal.exited_signal", signal = signal);
        }
    }
    match status.code() {
        Some(code) if code != 0 => ts!("terminal.exited_code", code = code),
        _ => ts!("terminal.exited"),
    }
}

// `#[cfg(test)]` spelled exactly like this: the i18n literal check (src/i18n.rs) only recognises this form
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;

    #[cfg(unix)]
    #[test]
    fn clean_exit_and_unknown_status_keep_the_plain_line() {
        assert_eq!(exit_notice(Some(ExitStatus::from_raw(0))), "[进程已退出]");
        assert_eq!(exit_notice(None), "[进程已退出]");
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_exit_shows_the_code() {
        // wait(2) encodes an exit code in the high byte: exit(3) -> 3 << 8
        let s = exit_notice(Some(ExitStatus::from_raw(3 << 8)));
        assert!(s.contains("退出码 3"), "{s}");
    }

    #[cfg(unix)]
    #[test]
    fn killed_by_signal_shows_the_signal_not_a_code() {
        // low 7 bits = terminating signal: 9 = SIGKILL
        let s = exit_notice(Some(ExitStatus::from_raw(9)));
        assert!(s.contains("信号 9"), "{s}");
        assert!(!s.contains("退出码"), "被信号杀死时没有退出码：{s}");
    }
}
