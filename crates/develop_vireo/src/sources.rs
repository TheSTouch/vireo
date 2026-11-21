//! Vireo's sources on this machine: where they are, whether they are there,
//! downloading them, and the tools needed to build them.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use util::command::new_command;

pub const REPOSITORY_URL: &str = "https://github.com/TheSTouch/vireo.git";
pub const BUILD_DOCS_URL: &str =
    "https://github.com/TheSTouch/vireo/blob/main/docs/src/development/macos.md";
/// The key-value store entry holding a folder the person chose instead of the default.
pub const SOURCES_PATH_KEY: &str = "develop_vireo_sources_path";

/// `~/Vireo`, where the sources are downloaded unless the person picks another folder.
pub fn default_sources_path() -> PathBuf {
    util::paths::home_dir().join("Vireo")
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourcesState {
    Checking,
    /// Nothing at the path yet.
    Missing,
    /// The path exists but is not a git checkout.
    NotACheckout,
    Present(Checkout),
    Downloading,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Checkout {
    pub branch: String,
    pub commit: String,
    /// Files with uncommitted changes.
    pub local_changes: usize,
}

impl SourcesState {
    pub fn is_present(&self) -> bool {
        matches!(self, SourcesState::Present(_))
    }
}

pub async fn check(path: &Path) -> SourcesState {
    if !path.exists() {
        return SourcesState::Missing;
    }
    if !path.join(".git").exists() {
        return SourcesState::NotACheckout;
    }
    match read_checkout(path).await {
        Ok(checkout) => SourcesState::Present(checkout),
        Err(error) => SourcesState::Failed(format!("{error:#}")),
    }
}

async fn read_checkout(path: &Path) -> Result<Checkout> {
    let branch = git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    let commit = git(path, &["rev-parse", "--short", "HEAD"]).await?;
    let status = git(path, &["status", "--porcelain"]).await?;
    Ok(Checkout {
        branch,
        commit,
        local_changes: status
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
    })
}

/// Clones the repository into `path`. Blobs are fetched on demand, which
/// makes the download much smaller than the full history.
pub async fn download(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let output = new_command("git")
        .arg("clone")
        .arg("--filter=blob:none")
        .arg(REPOSITORY_URL)
        .arg(path)
        .output()
        .await
        .context("running git clone")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!(last_line(&output.stderr)))
    }
}

async fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = new_command("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(anyhow!(last_line(&output.stderr)))
    }
}

fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("failed with no message")
        .trim()
        .to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prerequisite {
    pub name: &'static str,
    /// The tool's version line, or `None` when it was not found.
    pub found: Option<String>,
}

/// The tools a build needs: git, Rust and, on macOS, Xcode's command line tools.
pub async fn check_prerequisites() -> Vec<Prerequisite> {
    let mut checks: Vec<(&'static str, &'static str, &'static [&'static str])> = vec![
        ("Git", "git", &["--version"]),
        ("Rust (cargo)", "cargo", &["--version"]),
    ];
    if cfg!(target_os = "macos") {
        checks.push(("Xcode command line tools", "xcrun", &["--show-sdk-version"]));
    }
    let mut prerequisites = Vec::with_capacity(checks.len());
    for (name, program, args) in checks {
        let found = match new_command(program).args(args).output().await {
            Ok(output) if output.status.success() => Some(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            ),
            _ => None,
        };
        prerequisites.push(Prerequisite { name, found });
    }
    prerequisites
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_keep_the_last_line() {
        assert_eq!(
            last_line(b"Cloning into 'x'...\nfatal: repository not found\n\n"),
            "fatal: repository not found"
        );
        assert_eq!(last_line(b""), "failed with no message");
    }

    #[gpui::test]
    async fn checks_missing_folders_and_non_checkouts(_cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            check(&dir.path().join("absent")).await,
            SourcesState::Missing
        );
        assert_eq!(check(dir.path()).await, SourcesState::NotACheckout);
    }
}
