//! Replacing the running app bundle (#200).
//!
//! Why split from `mod.rs`: the decision "can we replace in place, and with what" and the helper script are
//! pure and testable; getting them wrong means either nagging users who cannot update in place or, worse,
//! clobbering a Homebrew-managed install / a bundle we have no right to write.
//!
//! Safety properties: the downloaded installer's SHA-256 must equal the digest from the release API; the staged
//! bundle's version must equal the release version; the old bundle is moved aside (not deleted) until the new
//! one is in place and is restored if the swap fails; files fetched by our own HTTP client carry no quarantine
//! flag, and the helper strips it anyway.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use makit_core::update::{self as core_update, ReleaseInfo};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub version: String,
    pub url: String,
    pub sha256: String,
    /// `/Applications/makit.app`
    pub target: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Managed by Homebrew: show / copy this command instead of replacing the bundle behind brew's back
    Brew { command: String },
    /// Open the release page and let the user install
    OpenPage,
    InPlace(Job),
}

/// `…/makit.app/Contents/MacOS/makit` → `…/makit.app`
pub fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Pure decision from what the environment looks like
pub fn decide(release: &ReleaseInfo, exe: Option<&Path>, dev: bool, brew_dir: Option<&str>, parent_writable: impl Fn(&Path) -> bool) -> Plan {
    if brew_dir.is_some() {
        return Plan::Brew { command: "brew upgrade --cask makit".into() };
    }
    let (Some(asset), Some(bundle)) = (&release.asset, exe.and_then(app_bundle_of)) else { return Plan::OpenPage };
    let Some(sha256) = asset.sha256.clone() else { return Plan::OpenPage };
    let parent_ok = bundle.parent().is_some_and(&parent_writable);
    if dev || !parent_ok {
        return Plan::OpenPage;
    }
    Plan::InPlace(Job { version: release.version.clone(), url: asset.url.clone(), sha256, target: bundle })
}

pub fn plan(release: &ReleaseInfo, dev: bool) -> Plan {
    let exe = std::env::current_exe().ok();
    let brew = core_update::brew_cask_dir(|p| Path::new(p).is_dir());
    decide(release, exe.as_deref(), dev, brew, |dir| {
        // Probe by creating a file: permission bits and ACLs lie more often than a real write does
        let probe = dir.join(format!(".makit-update-probe-{}", std::process::id()));
        std::fs::write(&probe, b"").map(|_| std::fs::remove_file(&probe).is_ok()).unwrap_or(false)
    })
}

/// The detached helper: waits for `$1` (the pid) to exit, swaps `$2` (target) for `$3` (staged), rolls back on
/// failure, strips the quarantine flag and relaunches
pub const HELPER: &str = r#"#!/bin/sh
pid="$1"; target="$2"; staged="$3"
n=0
while kill -0 "$pid" 2>/dev/null; do
  n=$((n+1)); [ "$n" -gt 300 ] && exit 1
  sleep 0.2
done
backup="$target.old.$$"
if mv "$target" "$backup"; then
  if mv "$staged" "$target"; then
    /usr/bin/xattr -dr com.apple.quarantine "$target" 2>/dev/null
    rm -rf "$backup"
  else
    mv "$backup" "$target"
  fi
fi
/usr/bin/open "$target"
"#;

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Download, verify, stage, and start the helper. On success the caller quits the app
pub fn stage_and_launch(job: &Job) -> Result<(), String> {
    let work = std::env::temp_dir().join(format!("makit-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = stage(job, &work);
    // The mount (if any) is detached inside `stage`; the downloaded dmg is no longer needed either way
    let _ = std::fs::remove_file(work.join("update.dmg"));
    match result {
        Ok(staged) => launch_helper(job, &work, &staged),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&work);
            Err(e)
        }
    }
}

fn stage(job: &Job, work: &Path) -> Result<PathBuf, String> {
    let dmg = work.join("update.dmg");
    core_update::download(&job.url, &dmg)?;
    let sum = run(Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(&dmg))?;
    let got = sum.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if got != job.sha256 {
        return Err("the downloaded file does not match the published checksum".into());
    }
    let mount = work.join("mnt");
    std::fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
    run(Command::new("/usr/bin/hdiutil").args(["attach", "-nobrowse", "-readonly", "-noverify", "-mountpoint"]).arg(&mount).arg(&dmg))?;
    let copied = (|| {
        let src = mount.join("makit.app");
        // Stage next to the target so the final move is a same-volume rename
        let staged = job.target.with_file_name(format!(".makit-update-{}.app", std::process::id()));
        let _ = std::fs::remove_dir_all(&staged);
        run(Command::new("/usr/bin/ditto").arg(&src).arg(&staged))?;
        Ok::<PathBuf, String>(staged)
    })();
    let _ = run(Command::new("/usr/bin/hdiutil").args(["detach", "-quiet"]).arg(&mount));
    let staged = copied?;
    let version = run(Command::new("/usr/libexec/PlistBuddy").args(["-c", "Print :CFBundleShortVersionString"]).arg(staged.join("Contents/Info.plist")))?;
    if version != job.version {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(format!("unexpected version {version} in the installer"));
    }
    Ok(staged)
}

fn launch_helper(job: &Job, work: &Path, staged: &Path) -> Result<(), String> {
    let script = work.join("swap.sh");
    std::fs::write(&script, HELPER).map_err(|e| e.to_string())?;
    Command::new("/bin/sh")
        .arg(&script)
        .arg(std::process::id().to_string())
        .arg(&job.target)
        .arg(staged)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use makit_core::update::Asset;

    fn release(with_asset: bool, with_digest: bool) -> ReleaseInfo {
        ReleaseInfo {
            version: "0.1.4".into(),
            notes: String::new(),
            page_url: "https://example.invalid/r".into(),
            asset: with_asset.then(|| Asset {
                name: "makit-0.1.4.dmg".into(),
                url: "https://example.invalid/makit-0.1.4.dmg".into(),
                sha256: with_digest.then(|| "ab".repeat(32)),
                size: 1,
            }),
        }
    }

    const EXE: &str = "/Applications/makit.app/Contents/MacOS/makit";

    #[test]
    fn bundle_is_found_from_the_executable_path() {
        assert_eq!(app_bundle_of(Path::new(EXE)), Some(PathBuf::from("/Applications/makit.app")));
        assert_eq!(app_bundle_of(Path::new("/tmp/target/release/makit-native")), None, "cargo run / bare binary");
    }

    #[test]
    fn a_normal_install_updates_in_place() {
        let p = decide(&release(true, true), Some(Path::new(EXE)), false, None, |_| true);
        assert!(matches!(p, Plan::InPlace(j) if j.target == Path::new("/Applications/makit.app") && j.version == "0.1.4"));
    }

    #[test]
    fn brew_managed_installs_get_the_brew_command() {
        let p = decide(&release(true, true), Some(Path::new(EXE)), false, Some("/usr/local/Caskroom/makit"), |_| true);
        assert_eq!(p, Plan::Brew { command: "brew upgrade --cask makit".into() });
    }

    #[test]
    fn anything_unsafe_falls_back_to_the_release_page() {
        let ok = |_: &Path| true;
        assert_eq!(decide(&release(true, true), Some(Path::new(EXE)), true, None, ok), Plan::OpenPage, "dev instance");
        assert_eq!(decide(&release(true, true), Some(Path::new("/tmp/makit-native")), false, None, ok), Plan::OpenPage, "not an app bundle");
        assert_eq!(decide(&release(true, true), Some(Path::new(EXE)), false, None, |_| false), Plan::OpenPage, "directory not writable");
        assert_eq!(decide(&release(false, false), Some(Path::new(EXE)), false, None, ok), Plan::OpenPage, "no installer in the release");
        assert_eq!(decide(&release(true, false), Some(Path::new(EXE)), false, None, ok), Plan::OpenPage, "no checksum to verify against");
    }

    #[test]
    fn helper_script_rolls_back_and_cleans_up() {
        assert!(HELPER.contains("kill -0"), "waits for the app to exit first");
        assert!(HELPER.contains("mv \"$backup\" \"$target\""), "restores the old bundle when the swap fails");
        assert!(HELPER.contains("com.apple.quarantine"));
    }
}
