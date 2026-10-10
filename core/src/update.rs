//! Update check (#200): version parsing, release metadata parsing and the "should we tell the user" decision.
//!
//! Why a leaf module with its own tests: the failure modes are silent. A wrong comparison means the pill never
//! shows (`0.1.10` sorted before `0.1.9`) or shows for a version the user already skipped; a wrong asset pick
//! downloads the wrong file. The only data sent over the network is a plain GET of release metadata: no session
//! content, paths or project names (the welcome card promises nothing is uploaded).

use std::cmp::Ordering;
use std::process::Command;

use serde_json::Value;

/// `major.minor.patch` with an optional pre-release suffix (`0.2.0-rc.1`); a leading `v` is accepted
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches('v');
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some((c, _)) => (c, None),
            None => (s, None),
        };
        let mut it = core.split('.');
        let mut num = || it.next()?.parse::<u64>().ok();
        let (major, minor, patch) = (num()?, num()?, num()?);
        if it.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater, // a release is newer than its own pre-release
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Lower-case hex SHA-256 from the release API (`digest: "sha256:…"`); None when the API did not provide one
    pub sha256: Option<String>,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// Without the leading `v`
    pub version: String,
    pub notes: String,
    pub page_url: String,
    /// The installer to download; None when the source only knows the version (Gitee tags)
    pub asset: Option<Asset>,
}

const GITHUB_LATEST: &str = "https://api.github.com/repos/nicholas-hoult/makit/releases/latest";
const GITEE_TAGS: &str = "https://gitee.com/api/v5/repos/nicholas-hoult/makit/tags";

/// `releases/latest` of the GitHub API. Picks `makit-<version>.dmg` as the installer
pub fn parse_github_latest(json: &str) -> Result<ReleaseInfo, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    let tag = v.get("tag_name").and_then(Value::as_str).ok_or("no tag_name")?;
    let version = Version::parse(tag).ok_or_else(|| format!("unparseable tag {tag}"))?;
    let _ = version;
    let version = tag.trim_start_matches('v').to_string();
    let asset = v.get("assets").and_then(Value::as_array).and_then(|assets| {
        let want = format!("makit-{version}.dmg");
        assets.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(want.as_str())).map(|a| Asset {
            name: want.clone(),
            url: a.get("browser_download_url").and_then(Value::as_str).unwrap_or_default().to_string(),
            sha256: a.get("digest").and_then(Value::as_str).and_then(|d| d.strip_prefix("sha256:")).map(|h| h.to_ascii_lowercase()),
            size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    Ok(ReleaseInfo {
        version,
        notes: v.get("body").and_then(Value::as_str).unwrap_or_default().to_string(),
        page_url: v.get("html_url").and_then(Value::as_str).unwrap_or("https://github.com/nicholas-hoult/makit/releases").to_string(),
        asset: asset.filter(|a| !a.url.is_empty()),
    })
}

/// Gitee `tags`: only versions, no installer. Picks the highest non-pre-release tag
pub fn parse_gitee_tags(json: &str) -> Result<ReleaseInfo, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    let best = v
        .as_array()
        .ok_or("not an array")?
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .filter_map(|n| Version::parse(n).filter(|v| v.pre.is_none()).map(|v| (v, n.trim_start_matches('v').to_string())))
        .max_by(|a, b| a.0.cmp(&b.0))
        .ok_or("no release tag")?;
    Ok(ReleaseInfo {
        page_url: format!("https://gitee.com/nicholas-hoult/makit/releases/tag/v{}", best.1),
        version: best.1,
        notes: String::new(),
        asset: None,
    })
}

/// Should the pill show: the release is strictly newer than the running version and was not skipped
pub fn is_available(current: &str, latest: &str, skipped: &str) -> bool {
    match (Version::parse(current), Version::parse(latest)) {
        (Some(c), Some(l)) => l > c && skipped.trim_start_matches('v') != latest.trim_start_matches('v'),
        _ => false,
    }
}

/// Whether a periodic check is due
pub fn check_due(last_check_secs: u64, now_secs: u64, interval_secs: u64) -> bool {
    last_check_secs == 0 || now_secs.saturating_sub(last_check_secs) >= interval_secs
}

/// Homebrew installs casks under a Caskroom directory; if one named `makit` exists the app is brew-managed and
/// should be updated with `brew upgrade --cask makit` instead of replacing the bundle behind brew's back
pub fn brew_cask_dir(exists: impl Fn(&str) -> bool) -> Option<&'static str> {
    ["/opt/homebrew/Caskroom/makit", "/usr/local/Caskroom/makit"].into_iter().find(|p| exists(p))
}

/// Checks GitHub first (it has the installer), then Gitee (version only; reachable where GitHub is not)
pub fn fetch_latest() -> Result<ReleaseInfo, String> {
    match curl_text(GITHUB_LATEST).and_then(|t| parse_github_latest(&t)) {
        Ok(r) => Ok(r),
        Err(github) => curl_text(GITEE_TAGS).and_then(|t| parse_gitee_tags(&t)).map_err(|gitee| format!("github: {github}; gitee: {gitee}")),
    }
}

fn curl_text(url: &str) -> Result<String, String> {
    let out = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "10", "-H", "Accept: application/vnd.github+json", "-A", concat!("makit/", env!("CARGO_PKG_VERSION")), url])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl exit {}", out.status.code().unwrap_or(-1)));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// Downloads `url` to `dest` (follows redirects, 10 min cap)
pub fn download(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let st = Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "600", "-A", concat!("makit/", env!("CARGO_PKG_VERSION")), "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("download failed (curl exit {})", st.code().unwrap_or(-1))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_not_as_text() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.1.10") > v("0.1.9"));
        assert!(v("0.2.0") > v("0.1.99"));
        assert!(v("v1.0.0") > v("0.9.9"));
        assert_eq!(v("0.1.3"), v("v0.1.3"));
    }

    #[test]
    fn a_release_is_newer_than_its_own_prerelease() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.2.0") > v("0.2.0-rc.1"));
        assert!(v("0.2.0-rc.2") > v("0.2.0-rc.1"));
    }

    #[test]
    fn garbage_versions_are_rejected() {
        for s in ["", "1", "1.2", "1.2.3.4", "a.b.c", "latest"] {
            assert!(Version::parse(s).is_none(), "{s}");
        }
    }

    #[test]
    fn availability_needs_newer_and_not_skipped() {
        assert!(is_available("0.1.2", "0.1.3", ""));
        assert!(!is_available("0.1.3", "0.1.3", ""), "same version");
        assert!(!is_available("0.1.4", "0.1.3", ""), "older release");
        assert!(!is_available("0.1.2", "0.1.3", "0.1.3"), "skipped");
        assert!(!is_available("0.1.2", "v0.1.3", "v0.1.3"), "skipped, with a v prefix");
        assert!(is_available("0.1.2", "0.1.4", "0.1.3"), "a later release is not covered by an earlier skip");
        assert!(!is_available("dev", "0.1.3", ""), "unparseable running version never nags");
    }

    #[test]
    fn periodic_check_is_due_after_the_interval() {
        assert!(check_due(0, 1000, 86400), "never checked");
        assert!(!check_due(1000, 1000 + 3600, 86400));
        assert!(check_due(1000, 1000 + 86400, 86400));
        assert!(!check_due(5000, 100, 86400), "clock went backwards: do not hammer the API");
    }

    const GITHUB: &str = r#"{
      "tag_name": "v0.1.3", "html_url": "https://github.com/nicholas-hoult/makit/releases/tag/v0.1.3", "body": "- faster",
      "assets": [
        {"name": "notes.txt", "browser_download_url": "https://example.invalid/notes.txt", "size": 5},
        {"name": "makit-0.1.3.dmg", "browser_download_url": "https://github.com/nicholas-hoult/makit/releases/download/v0.1.3/makit-0.1.3.dmg",
         "digest": "sha256:31D709158A92159D05B04D52D56DFD09D93C1EAA45955D9DBE2A6DD31EF07E40", "size": 15710964}
      ]}"#;

    #[test]
    fn github_latest_picks_the_dmg_and_lowercases_the_digest() {
        let r = parse_github_latest(GITHUB).unwrap();
        assert_eq!(r.version, "0.1.3");
        assert!(r.notes.contains("faster"));
        let a = r.asset.unwrap();
        assert_eq!(a.name, "makit-0.1.3.dmg");
        assert_eq!(a.sha256.as_deref(), Some("31d709158a92159d05b04d52d56dfd09d93c1eaa45955d9dbe2a6dd31ef07e40"));
        assert_eq!(a.size, 15710964);
    }

    #[test]
    fn github_latest_without_the_installer_still_gives_a_version() {
        let r = parse_github_latest(r#"{"tag_name":"v0.1.4","assets":[]}"#).unwrap();
        assert_eq!(r.version, "0.1.4");
        assert!(r.asset.is_none());
    }

    #[test]
    fn github_latest_rejects_non_release_json() {
        assert!(parse_github_latest(r#"{"message":"Not Found"}"#).is_err());
        assert!(parse_github_latest("<html>").is_err());
    }

    #[test]
    fn gitee_tags_pick_the_highest_stable_tag() {
        let r = parse_gitee_tags(r#"[{"name":"v0.1.9"},{"name":"v0.1.10"},{"name":"v0.2.0-rc.1"},{"name":"backup/pre-rewrite"}]"#).unwrap();
        assert_eq!(r.version, "0.1.10");
        assert!(r.asset.is_none());
        assert!(r.page_url.contains("gitee.com"));
    }

    #[test]
    fn brew_managed_when_a_caskroom_dir_exists() {
        assert_eq!(brew_cask_dir(|p| p == "/opt/homebrew/Caskroom/makit"), Some("/opt/homebrew/Caskroom/makit"));
        assert_eq!(brew_cask_dir(|p| p == "/usr/local/Caskroom/makit"), Some("/usr/local/Caskroom/makit"));
        assert_eq!(brew_cask_dir(|_| false), None);
    }
}
