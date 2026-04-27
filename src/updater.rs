//! Background update checker that queries the GitHub releases API on startup.

use semver::Version;
use std::sync::Arc;
use tokio::sync::watch;

/// Holds the result of an update check.
#[derive(Clone, Debug)]
pub enum UpdateStatus {
    /// Still checking.
    Checking,
    /// A newer version is available.
    Available {
        current: String,
        latest: String,
        html_url: String,
    },
    /// Already on the latest (or newer) version.
    UpToDate,
    /// The check failed (network error, rate-limited, etc.).
    Failed(String),
}

/// A handle returned to the UI so it can poll the status.
#[derive(Clone)]
pub struct UpdateHandle {
    pub rx: watch::Receiver<UpdateStatus>,
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_API_URL: &str =
    "https://api.github.com/repos/Nexetry/e-sh/releases/latest";

/// Spawns a background task that checks for updates and returns a handle.
pub fn spawn_update_check(rt: &tokio::runtime::Handle) -> UpdateHandle {
    let (tx, rx) = watch::channel(UpdateStatus::Checking);
    let tx = Arc::new(tx);

    rt.spawn(async move {
        match check_latest().await {
            Ok(status) => {
                let _ = tx.send(status);
            }
            Err(e) => {
                tracing::warn!(error = %e, "update check failed");
                let _ = tx.send(UpdateStatus::Failed(e.to_string()));
            }
        }
    });

    UpdateHandle { rx }
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

async fn check_latest() -> anyhow::Result<UpdateStatus> {
    let client = reqwest::Client::builder()
        .user_agent(format!("e-sh/{CURRENT_VERSION}"))
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let release: GithubRelease = client
        .get(GITHUB_API_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let tag = release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name);
    let latest = Version::parse(tag)
        .map_err(|e| anyhow::anyhow!("cannot parse remote version '{tag}': {e}"))?;
    let current = Version::parse(CURRENT_VERSION)
        .map_err(|e| anyhow::anyhow!("cannot parse local version: {e}"))?;

    if latest > current {
        Ok(UpdateStatus::Available {
            current: current.to_string(),
            latest: latest.to_string(),
            html_url: release.html_url,
        })
    } else {
        Ok(UpdateStatus::UpToDate)
    }
}
