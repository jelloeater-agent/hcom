//! Resolve the user's login+interactive shell environment for nested launches.
//!
//! This follows the editor-style "resolve shell env" pattern: run the user's
//! shell out-of-band, cache the captured env, and fail open to the caller on
//! any problem.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const CACHE_VERSION: u32 = 2;
const SHELL_TIMEOUT: Duration = Duration::from_secs(10);
const MARKER_VAR: &str = "HCOM_SHELL_ENV_MARKER";
const RC_FILES: &[&str] = &[
    ".zshrc",
    ".zprofile",
    ".zshenv",
    ".bashrc",
    ".bash_profile",
    ".profile",
];

/// Resolve the user's login+interactive shell environment, out-of-band, cached.
/// Returns None on any failure so callers can fall back to parent inheritance.
pub fn resolved_shell_env() -> Option<HashMap<String, String>> {
    let cache_path = crate::paths::hcom_path(&["shell_env.json"]);
    let home = dirs::home_dir().or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    let rc_mtime = rc_mtime_key_for_home(&home).ok()?;
    let now = epoch_secs(SystemTime::now())?;

    if let Some(entry) = read_cache(&cache_path)
        && cache_is_fresh(&entry, rc_mtime, now)
    {
        return Some(entry.env);
    }

    let env = resolve_shell_env_uncached()?;
    let entry = ShellEnvCache {
        version: CACHE_VERSION,
        rc_mtime,
        written_at: now,
        env: env.clone(),
    };
    let _ = write_cache(&cache_path, &entry);
    Some(env)
}

fn resolve_shell_env_uncached() -> Option<HashMap<String, String>> {
    let shell = shell_path()?;
    if unsupported_shell(&shell) {
        return None;
    }

    let marker = format!("hcom-shell-env-{}", uuid::Uuid::new_v4());
    let cmd = format!("printf %s \"${MARKER_VAR}\"; env -0; printf %s \"${MARKER_VAR}\"");
    let output = timed_shell_output(&shell, &cmd, &marker)?;
    parse_shell_env_output(&output.stdout, &marker, MARKER_VAR)
}

fn shell_path() -> Option<PathBuf> {
    if let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(shell));
    }
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from("/bin/zsh"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(PathBuf::from("/bin/bash"))
    }
}

fn unsupported_shell(shell: &Path) -> bool {
    let Some(name) = shell.file_name().and_then(|s| s.to_str()) else {
        return true;
    };
    matches!(name, "fish" | "pwsh" | "powershell" | "nu")
}

struct ShellOutput {
    stdout: Vec<u8>,
}

fn timed_shell_output(shell: &Path, cmd: &str, marker: &str) -> Option<ShellOutput> {
    let mut child = Command::new(shell)
        .arg("-lic")
        .arg(cmd)
        .env_clear()
        .envs(clean_shell_seed_env(shell))
        .env(MARKER_VAR, marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut stdout);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr);
                }
                if !status.success() {
                    return None;
                }
                return Some(ShellOutput { stdout });
            }
            Ok(None) => {
                if start.elapsed() >= SHELL_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ShellEnvCache {
    #[serde(default)]
    version: u32,
    rc_mtime: u64,
    written_at: u64,
    env: HashMap<String, String>,
}

fn read_cache(path: &Path) -> Option<ShellEnvCache> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_cache(path: &Path, entry: &ShellEnvCache) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
    let tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))?;
    fs::write(tmp.path(), content)?;
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o600))?;
    tmp.persist(path).map_err(std::io::Error::other)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn cache_is_fresh(entry: &ShellEnvCache, rc_mtime: u64, now: u64) -> bool {
    entry.version == CACHE_VERSION
        && entry.rc_mtime == rc_mtime
        && now
            .checked_sub(entry.written_at)
            .is_some_and(|age| age <= CACHE_TTL.as_secs())
}

fn clean_shell_seed_env(shell: &Path) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for key in ["HOME", "USER", "LOGNAME", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            env.insert(key.to_string(), value.to_string_lossy().to_string());
        }
    }
    env.insert("SHELL".to_string(), shell.to_string_lossy().to_string());
    env.insert(
        "PATH".to_string(),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
    );
    env
}

fn rc_mtime_key_for_home(home: &Path) -> std::io::Result<u64> {
    let mut max = 0;
    for file in RC_FILES {
        let path = home.join(file);
        if let Ok(meta) = fs::metadata(path) {
            let modified = meta.modified()?;
            max = max.max(epoch_secs(modified).unwrap_or(0));
        }
    }
    Ok(max)
}

fn epoch_secs(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn parse_shell_env_output(
    stdout: &[u8],
    marker: &str,
    marker_var: &str,
) -> Option<HashMap<String, String>> {
    let marker_bytes = marker.as_bytes();
    let first = find_bytes(stdout, marker_bytes)?;
    let after_first = first + marker_bytes.len();
    let second_rel = rfind_bytes(&stdout[after_first..], marker_bytes)?;
    let body = &stdout[after_first..after_first + second_rel];

    let mut env = HashMap::new();
    for entry in body.split(|b| *b == b'\0') {
        if entry.is_empty() {
            continue;
        }
        let Some(eq) = entry.iter().position(|b| *b == b'=') else {
            continue;
        };
        let key = String::from_utf8(entry[..eq].to_vec()).ok()?;
        if key.starts_with("HCOM_") || key == marker_var {
            continue;
        }
        let value = String::from_utf8(entry[eq + 1..].to_vec()).ok()?;
        env.insert(key, value);
    }
    Some(env)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn parse_shell_env_output_extracts_between_markers_and_strips_hcom() {
        let marker = "MARKER";
        let stdout = b"noiseMARKER\0a=1\0b=two\nlines\0HCOM_PROCESS_ID=pid\0HCOM_SHELL_ENV_MARKER=MARKER\0MARKERtail";

        let env = parse_shell_env_output(stdout, marker, MARKER_VAR).unwrap();

        assert_eq!(env.get("a").map(String::as_str), Some("1"));
        assert_eq!(env.get("b").map(String::as_str), Some("two\nlines"));
        assert!(!env.contains_key("HCOM_PROCESS_ID"));
        assert!(!env.contains_key(MARKER_VAR));
    }

    #[test]
    fn parse_shell_env_output_ignores_marker_value_inside_env_body() {
        let marker = "MARKER";
        let stdout = b"MARKERHCOM_SHELL_ENV_MARKER=MARKER\0a=1\0MARKER";

        let env = parse_shell_env_output(stdout, marker, MARKER_VAR).unwrap();

        assert_eq!(env.get("a").map(String::as_str), Some("1"));
        assert!(!env.contains_key(MARKER_VAR));
    }

    #[test]
    fn parse_shell_env_output_rejects_missing_marker() {
        assert!(parse_shell_env_output(b"a=1\0b=2", "MARKER", MARKER_VAR).is_none());
    }

    #[test]
    fn cache_key_mtime_change_busts_cache() {
        let entry = ShellEnvCache {
            version: CACHE_VERSION,
            rc_mtime: 10,
            written_at: 100,
            env: HashMap::new(),
        };

        assert!(cache_is_fresh(&entry, 10, 101));
        assert!(!cache_is_fresh(&entry, 11, 101));
    }

    #[test]
    fn cache_version_change_busts_cache() {
        let entry = ShellEnvCache {
            version: CACHE_VERSION - 1,
            rc_mtime: 10,
            written_at: 100,
            env: HashMap::new(),
        };

        assert!(!cache_is_fresh(&entry, 10, 101));
    }

    #[test]
    fn cache_key_ttl_busts_cache() {
        let entry = ShellEnvCache {
            version: CACHE_VERSION,
            rc_mtime: 10,
            written_at: 100,
            env: HashMap::new(),
        };

        assert!(cache_is_fresh(&entry, 10, 100 + CACHE_TTL.as_secs()));
        assert!(!cache_is_fresh(&entry, 10, 101 + CACHE_TTL.as_secs()));
    }

    #[test]
    fn cache_write_uses_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shell_env.json");
        let entry = ShellEnvCache {
            version: CACHE_VERSION,
            rc_mtime: 1,
            written_at: 2,
            env: HashMap::from([("OPENAI_API_KEY".to_string(), "sk-test".to_string())]),
        };

        write_cache(&path, &entry).unwrap();

        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    #[serial]
    fn clean_shell_seed_env_excludes_parent_tool_contamination() {
        unsafe { std::env::set_var("CODEX_CI", "1") };
        unsafe { std::env::set_var("NO_COLOR", "1") };
        unsafe { std::env::set_var("CARGO_MANIFEST_DIR", "/tmp/hcom") };

        let env = clean_shell_seed_env(Path::new("/bin/zsh"));

        assert_eq!(env.get("SHELL").map(String::as_str), Some("/bin/zsh"));
        assert!(!env.contains_key("CODEX_CI"));
        assert!(!env.contains_key("NO_COLOR"));
        assert!(!env.contains_key("CARGO_MANIFEST_DIR"));

        unsafe { std::env::remove_var("CODEX_CI") };
        unsafe { std::env::remove_var("NO_COLOR") };
        unsafe { std::env::remove_var("CARGO_MANIFEST_DIR") };
    }
}
