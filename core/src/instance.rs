//! Single-instance guard (#291): an exclusive lock on a file under the data directory, held for the life of the
//! process.
//!
//! Why a lock file and not "look for another process": the lock is released by the kernel when the process ends,
//! including a crash or a kill, so it never goes stale; a pid file would. And it works the same when the app is started
//! from a different path (a dmg, Downloads), which is one way two copies end up running.
//!
//! Why it is its own module with tests: getting it wrong is either two instances fighting over the hook socket and
//! the state files (the lock never taken), or an app that silently refuses to start (the lock never released, or a
//! failure to lock treated as "someone else has it").

use std::fs::File;
use std::path::Path;
use std::time::Duration;

/// Held for as long as this value lives; dropping it (or the process ending) releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

#[derive(Debug)]
pub enum Acquire {
    /// We are the only instance
    Acquired(InstanceLock),
    /// Another instance holds the lock (and still did after `wait`)
    Held,
    /// The lock could not be taken for a reason unrelated to another instance (unwritable directory, unsupported
    /// platform). The caller should start normally: a broken guard must not stop the app from opening.
    Unavailable(String),
}

/// Take the lock at `path`, waiting up to `wait` for a previous instance that is still shutting down.
#[cfg(unix)]
pub fn acquire(path: &Path, wait: Duration) -> Acquire {
    use std::os::unix::io::AsRawFd;
    use std::time::Instant;

    if let Some(dir) = path.parent() {
        // a failure here shows up as the open below failing, with a better message
        let _ = std::fs::create_dir_all(dir);
    }
    let file = match std::fs::OpenOptions::new().create(true).read(true).write(true).open(path) {
        Ok(f) => f,
        Err(e) => return Acquire::Unavailable(format!("{}: {e}", path.display())),
    };
    let start = Instant::now();
    loop {
        // SAFETY: plain syscall on a descriptor we own; the lock goes with the open file description
        // and is released when `file` is dropped or the process ends
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Acquire::Acquired(InstanceLock { _file: file });
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::WouldBlock {
            return Acquire::Unavailable(format!("flock {}: {err}", path.display()));
        }
        if start.elapsed() >= wait {
            return Acquire::Held;
        }
        std::thread::sleep(Duration::from_millis(100).min(wait));
    }
}

/// No guard on this platform yet (no `flock`): the caller starts normally
#[cfg(not(unix))]
pub fn acquire(_path: &Path, _wait: Duration) -> Acquire {
    Acquire::Unavailable("the single-instance guard is not implemented on this platform".into())
}

// the guard is unix-only for now (no flock on Windows), so are its tests
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    fn lock_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("makit-instance-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("instance.lock") // the directory does not exist yet: acquire must create it
    }

    #[test]
    fn the_first_instance_gets_the_lock_and_a_second_one_does_not() {
        let p = lock_path("two");
        let first = acquire(&p, Duration::ZERO);
        assert!(matches!(first, Acquire::Acquired(_)), "{first:?}");
        assert!(matches!(acquire(&p, Duration::ZERO), Acquire::Held), "second instance while the first lives");
    }

    #[test]
    fn dropping_the_lock_lets_the_next_instance_in() {
        let p = lock_path("drop");
        let first = acquire(&p, Duration::ZERO);
        assert!(matches!(first, Acquire::Acquired(_)));
        drop(first);
        assert!(matches!(acquire(&p, Duration::ZERO), Acquire::Acquired(_)), "released on drop, like on process exit");
    }

    #[test]
    fn a_previous_instance_that_is_still_quitting_is_waited_for() {
        // an update restart or "quit, then open again" starts the new process while the old one is still exiting
        let p = lock_path("wait");
        let first = acquire(&p, Duration::ZERO);
        assert!(matches!(first, Acquire::Acquired(_)));
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            drop(first);
        });
        let start = Instant::now();
        let second = acquire(&p, Duration::from_secs(5));
        releaser.join().unwrap();
        assert!(matches!(second, Acquire::Acquired(_)), "{second:?}");
        assert!(start.elapsed() < Duration::from_secs(3), "it must return as soon as the lock is free, not after the full wait");
    }

    #[test]
    fn it_gives_up_after_the_wait_when_the_other_instance_stays() {
        let p = lock_path("giveup");
        let _first = acquire(&p, Duration::ZERO);
        let start = Instant::now();
        assert!(matches!(acquire(&p, Duration::from_millis(300)), Acquire::Held));
        assert!(start.elapsed() >= Duration::from_millis(300), "it waited the full time before giving up");
    }

    #[test]
    fn a_directory_that_cannot_be_written_means_start_anyway() {
        // a file where the directory should be: neither the directory nor the lock file can be created
        let base = std::env::temp_dir().join(format!("makit-instance-test-{}-blocked", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_file(&base);
        std::fs::write(&base, b"x").unwrap();
        let got = acquire(&base.join("instance.lock"), Duration::ZERO);
        assert!(matches!(got, Acquire::Unavailable(_)), "must not be reported as another instance: {got:?}");
    }
}
