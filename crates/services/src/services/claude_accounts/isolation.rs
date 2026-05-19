//! Per-spawn credential isolation.
//!
//! Materializes a Claude account's OAuth credentials into a fresh temporary
//! directory and produces the env vars (`CLAUDE_CONFIG_DIR` + `HOME`) that point
//! the `claude` CLI at that directory. Drop removes the directory.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::json;
use uuid::Uuid;

use super::oauth::OauthAccountInfo;
use super::types::ClaudeOAuthCredentials;

/// Owns a per-spawn credential directory under `<asset_dir>/claude_spawn_tmp/<uuid>/`.
/// On drop, the directory is recursively removed.
pub struct TempCredentialDir {
    path: PathBuf,
}

impl TempCredentialDir {
    /// Create a new isolated credential dir and populate `.credentials.json` +
    /// `.claude.json` from the given account credentials.
    pub fn materialize(
        creds: &ClaudeOAuthCredentials,
        oauth_account: Option<&OauthAccountInfo>,
    ) -> std::io::Result<Self> {
        let parent = utils::assets::claude_spawn_tmp_dir();
        let path = parent.join(Uuid::new_v4().to_string());
        fs::create_dir_all(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o700);
            fs::set_permissions(&path, perms)?;
        }

        write_credentials_file(&path, creds)?;
        write_claude_json(&path, oauth_account)?;

        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Env vars that point the claude CLI at this isolated dir.
    /// `CLAUDE_CONFIG_DIR` is the documented knob; `HOME` is belt-and-suspenders
    /// to keep the CLI away from the user's real `~/.claude/`.
    pub fn env_vars(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();
        let p = self.path.to_string_lossy().into_owned();
        env.insert("CLAUDE_CONFIG_DIR".to_string(), p.clone());
        env.insert("HOME".to_string(), p);
        env
    }
}

impl Drop for TempCredentialDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_credentials_file(
    dir: &Path,
    creds: &ClaudeOAuthCredentials,
) -> std::io::Result<()> {
    let blob = json!({
        "claudeAiOauth": {
            "accessToken": creds.access_token,
            "refreshToken": creds.refresh_token,
            "expiresAt": creds.expires_at.timestamp_millis(),
            "scopes": creds.scopes,
        }
    });
    write_secret_json(&dir.join(".credentials.json"), &blob)
}

fn write_claude_json(
    dir: &Path,
    oauth_account: Option<&OauthAccountInfo>,
) -> std::io::Result<()> {
    let oauth_block = match oauth_account {
        Some(acct) => json!({
            "uuid": acct.uuid,
            "emailAddress": acct.email,
            "organizationUuid": acct.organization_uuid,
        }),
        None => json!({}),
    };
    let blob = json!({
        "hasCompletedOnboarding": true,
        "oauthAccount": oauth_block,
    });
    write_secret_json(&dir.join(".claude.json"), &blob)
}

fn write_secret_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    serde_json::to_writer_pretty(&file, value)?;
    file.sync_all()?;
    Ok(())
}

/// Delete any orphaned per-spawn directories older than 1 hour. Called from
/// the server startup sweep.
pub fn sweep_orphaned_tmp_dirs(now: DateTime<Utc>) {
    let dir = utils::assets::claude_spawn_tmp_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let cutoff = now - ChronoDuration::hours(1);
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let modified_chrono: DateTime<Utc> = modified.into();
        if modified_chrono < cutoff {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_creds() -> ClaudeOAuthCredentials {
        ClaudeOAuthCredentials {
            access_token: "sk-ant-oat01-fake".to_string(),
            refresh_token: "rt-fake".to_string(),
            expires_at: Utc::now() + ChronoDuration::hours(8),
            scopes: vec!["user:inference".to_string()],
        }
    }

    #[test]
    fn materialize_creates_directory_with_credentials_file() {
        let dir = TempCredentialDir::materialize(&fake_creds(), None).unwrap();
        assert!(dir.path().exists());
        assert!(dir.path().join(".credentials.json").exists());
        assert!(dir.path().join(".claude.json").exists());

        let env = dir.env_vars();
        assert_eq!(env.get("CLAUDE_CONFIG_DIR").unwrap(), &dir.path().to_string_lossy().into_owned());
        assert_eq!(env.get("HOME").unwrap(), &dir.path().to_string_lossy().into_owned());
    }

    #[cfg(unix)]
    #[test]
    fn credentials_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempCredentialDir::materialize(&fake_creds(), None).unwrap();
        let meta = fs::metadata(dir.path().join(".credentials.json")).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn drop_removes_directory() {
        let path = {
            let dir = TempCredentialDir::materialize(&fake_creds(), None).unwrap();
            dir.path().to_path_buf()
        };
        assert!(!path.exists(), "directory should be removed by Drop");
    }
}
