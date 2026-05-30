//! Cursor launch preprocessing: workspace trust markers.

use std::path::{Path, PathBuf};

/// Cursor stores per-workspace state under `~/.cursor/projects/<slug>`.
///
/// This mirrors Cursor's path slugging: path separators and punctuation become
/// dashes while ASCII letters, digits, underscores, and existing dashes survive.
pub(crate) fn cursor_project_slug(workspace: &Path) -> String {
    workspace
        .to_string_lossy()
        .split(std::path::MAIN_SEPARATOR)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                        ch
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Print/headless flags that would break hcom's PTY delivery model.
///
/// hcom always runs cursor-agent inside a PTY (interactive, or HeadlessPty for
/// background) — never `--print`. The `beforeSubmitPrompt`/`stop` hooks that
/// carry message delivery do **not** fire in `--print` mode, so a stray
/// `-p`/`--print` leaking in from `HCOM_CURSOR_ARGS` or a resumed instance's
/// baked `launch_args` would silently break delivery. `--stream-partial-output`
/// only works with `--print` + stream-json, so it's dead weight once `--print`
/// is gone. All three are booleans (no value token), so a plain filter is safe.
const CURSOR_PRINT_FLAGS: &[&str] = &["-p", "--print", "--stream-partial-output"];

/// Strip print/headless flags from a cursor-agent arg list (see
/// [`CURSOR_PRINT_FLAGS`]). Applied to both the launch-time arg merge and the
/// resume merge so neither path can drop cursor into one-shot `--print` mode.
pub(crate) fn strip_cursor_print_flags(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| !CURSOR_PRINT_FLAGS.contains(&t.as_str()))
        .cloned()
        .collect()
}

fn cursor_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".cursor")
        .join("projects")
}

pub(crate) fn cursor_trust_marker_path(workspace: &Path) -> PathBuf {
    let normalized = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    cursor_projects_dir()
        .join(cursor_project_slug(&normalized))
        .join(".workspace-trusted")
}

/// Pre-seed Cursor's workspace trust marker for PTY launches.
///
/// Cursor's `--trust` flag only works in print mode. hcom keeps Cursor
/// interactive inside a PTY, so the marker must exist before process startup.
pub(crate) fn ensure_cursor_workspace_trusted(workspace: &Path) -> anyhow::Result<()> {
    let normalized = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let marker = cursor_trust_marker_path(&normalized);
    if marker.exists() {
        return Ok(());
    }
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let trusted_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "trustedAt": trusted_at,
        "workspacePath": normalized.to_string_lossy(),
        "trustMethod": "hcom-launch",
    }))?;
    crate::paths::atomic_write_io(&marker, &content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_project_slug_matches_cursor_layout() {
        assert_eq!(
            cursor_project_slug(Path::new("/private/tmp/cursor-hook-probe.sdxJ")),
            "private-tmp-cursor-hook-probe-sdxJ"
        );
        assert_eq!(
            cursor_project_slug(Path::new("/Users/anno/Dev/hook-comms-public")),
            "Users-anno-Dev-hook-comms-public"
        );
    }

    #[test]
    fn strip_cursor_print_flags_drops_print_and_companions_keeps_rest() {
        let tokens: Vec<String> = [
            "--model",
            "composer-2.5",
            "-p",
            "--print",
            "--stream-partial-output",
            "--force",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            strip_cursor_print_flags(&tokens),
            vec![
                "--model".to_string(),
                "composer-2.5".to_string(),
                "--force".to_string()
            ]
        );
    }
}
