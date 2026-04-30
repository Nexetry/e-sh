//! Background update checker + self-updater that queries the GitHub releases API.

use semver::Version;
use std::path::PathBuf;
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
        /// Download URL for the platform-appropriate archive asset.
        asset_url: Option<String>,
    },
    /// Already on the latest (or newer) version.
    UpToDate,
    /// The check failed (network error, rate-limited, etc.).
    Failed(String),
}

/// Progress of a self-update download + install.
#[derive(Clone, Debug)]
pub enum ApplyStatus {
    Downloading,
    Installing,
    Done { restart_path: PathBuf },
    Failed(String),
}

/// A handle returned to the UI so it can poll the status.
pub struct UpdateHandle {
    pub rx: watch::Receiver<UpdateStatus>,
    _tx: Arc<watch::Sender<UpdateStatus>>,
}

/// A handle for tracking a self-update in progress.
pub struct ApplyHandle {
    pub rx: watch::Receiver<ApplyStatus>,
    _tx: Arc<watch::Sender<ApplyStatus>>,
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_API_URL: &str =
    "https://api.github.com/repos/Nexetry/e-sh/releases/latest";

/// Spawns a background task that checks for updates and returns a handle.
pub fn spawn_update_check(rt: &tokio::runtime::Handle) -> UpdateHandle {
    let (tx, rx) = watch::channel(UpdateStatus::Checking);
    let tx = Arc::new(tx);
    let tx2 = Arc::clone(&tx);

    rt.spawn(async move {
        match tokio::time::timeout(
            std::time::Duration::from_secs(20),
            check_latest(),
        )
        .await
        {
            Ok(Ok(status)) => {
                tracing::info!(?status, "update check completed");
                let _ = tx.send(status);
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "update check failed");
                let _ = tx.send(UpdateStatus::Failed(e.to_string()));
            }
            Err(_) => {
                tracing::warn!("update check timed out");
                let _ = tx.send(UpdateStatus::Failed("request timed out".to_string()));
            }
        }
    });

    UpdateHandle { rx, _tx: tx2 }
}

/// Spawns a background task that downloads and installs the update.
pub fn spawn_apply_update(rt: &tokio::runtime::Handle, asset_url: String) -> ApplyHandle {
    let (tx, rx) = watch::channel(ApplyStatus::Downloading);
    let tx = Arc::new(tx);
    let tx2 = Arc::clone(&tx);

    rt.spawn(async move {
        match apply_update(&tx, &asset_url).await {
            Ok(restart_path) => {
                let _ = tx.send(ApplyStatus::Done { restart_path });
            }
            Err(e) => {
                tracing::error!(error = %e, "self-update failed");
                let _ = tx.send(ApplyStatus::Failed(e.to_string()));
            }
        }
    });

    ApplyHandle { rx, _tx: tx2 }
}

fn build_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("e-sh/{CURRENT_VERSION}"))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .build()
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Return the asset name suffix we expect for this OS + arch.
fn platform_asset_suffix() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("macos-universal.tar.gz")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("linux-x86_64.tar.gz")
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some("windows-x86_64.zip")
    } else {
        None
    }
}

async fn check_latest() -> anyhow::Result<UpdateStatus> {
    let client = build_client()?;

    let response = client
        .get(GITHUB_API_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;

    let release: GithubRelease = response
        .error_for_status()?
        .json()
        .await?;

    tracing::info!("check_latest: parsed release tag={}", release.tag_name);
    let tag = release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name);
    let latest = Version::parse(tag)
        .map_err(|e| anyhow::anyhow!("cannot parse remote version '{tag}': {e}"))?;
    let current = Version::parse(CURRENT_VERSION)
        .map_err(|e| anyhow::anyhow!("cannot parse local version: {e}"))?;

    if latest > current {
        let asset_url = platform_asset_suffix().and_then(|suffix| {
            release
                .assets
                .iter()
                .find(|a| a.name.ends_with(suffix))
                .map(|a| a.browser_download_url.clone())
        });
        Ok(UpdateStatus::Available {
            current: current.to_string(),
            latest: latest.to_string(),
            html_url: release.html_url,
            asset_url,
        })
    } else {
        Ok(UpdateStatus::UpToDate)
    }
}

async fn apply_update(
    tx: &watch::Sender<ApplyStatus>,
    asset_url: &str,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;

    tracing::info!("apply_update: downloading {}", asset_url);
    let client = build_client()?;
    let bytes = client
        .get(asset_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await
        .context("downloading asset")?;
    tracing::info!("apply_update: downloaded {} bytes", bytes.len());

    let _ = tx.send(ApplyStatus::Installing);

    let current_exe = std::env::current_exe().context("locating current executable")?;
    tracing::info!("apply_update: current exe = {}", current_exe.display());

    // Determine the install target.
    // On macOS, if we're inside an .app bundle, replace the whole bundle.
    // Otherwise replace the binary in-place.
    let (install_dir, restart_path) = if cfg!(target_os = "macos") {
        find_macos_app_target(&current_exe)?
    } else {
        (current_exe.parent().unwrap().to_path_buf(), current_exe.clone())
    };

    // Extract archive to a temp directory next to the install target.
    let tmp_dir = install_dir.join(".e-sh-update-tmp");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir).ok();
    }
    std::fs::create_dir_all(&tmp_dir).context("creating temp dir")?;

    if asset_url.ends_with(".tar.gz") {
        let decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&tmp_dir).context("extracting tar.gz")?;
    } else if asset_url.ends_with(".zip") {
        let reader = std::io::Cursor::new(&bytes);
        let mut zip = zip::ZipArchive::new(reader).context("opening zip")?;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).context("reading zip entry")?;
            let out_path = tmp_dir.join(file.mangled_name());
            if file.is_dir() {
                std::fs::create_dir_all(&out_path).ok();
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                let mut out = std::fs::File::create(&out_path)
                    .with_context(|| format!("creating {}", out_path.display()))?;
                std::io::copy(&mut file, &mut out)?;
            }
        }
    } else {
        anyhow::bail!("unsupported archive format: {asset_url}");
    }

    tracing::info!("apply_update: extracted to {}", tmp_dir.display());

    // Perform the swap.
    let restart = if cfg!(target_os = "macos") {
        swap_macos_app(&tmp_dir, &install_dir, &restart_path)?
    } else {
        swap_binary(&tmp_dir, &current_exe)?
    };

    // Clean up.
    std::fs::remove_dir_all(&tmp_dir).ok();

    tracing::info!("apply_update: done, restart via {}", restart.display());
    Ok(restart)
}

/// On macOS, find the `.app` bundle root if we're inside one.
/// Returns (parent_of_app_bundle, path_to_relaunch).
fn find_macos_app_target(
    current_exe: &std::path::Path,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    // Typical: /Applications/e-sh.app/Contents/MacOS/e-sh
    // Walk up to find the .app directory.
    let mut path = current_exe.to_path_buf();
    loop {
        if path
            .extension()
            .map(|e| e == "app")
            .unwrap_or(false)
        {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!(".app has no parent dir"))?
                .to_path_buf();
            return Ok((parent, path));
        }
        if !path.pop() {
            break;
        }
    }
    // Not inside an .app bundle — fall back to binary replacement.
    Ok((
        current_exe.parent().unwrap().to_path_buf(),
        current_exe.to_path_buf(),
    ))
}

/// Replace a macOS `.app` bundle.
fn swap_macos_app(
    tmp_dir: &std::path::Path,
    install_dir: &std::path::Path,
    app_path: &std::path::Path,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;

    // Find the .app inside the extracted archive.
    let new_app = find_app_in_dir(tmp_dir)
        .ok_or_else(|| anyhow::anyhow!("no .app found in downloaded archive"))?;

    let app_name = app_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("bad app path"))?;
    let dest = install_dir.join(app_name);
    let backup = install_dir.join(format!(
        "{}.bak",
        app_name.to_string_lossy()
    ));

    // Backup current, move new in.
    if dest.exists() {
        if backup.exists() {
            std::fs::remove_dir_all(&backup).ok();
        }
        std::fs::rename(&dest, &backup)
            .with_context(|| format!("backing up {}", dest.display()))?;
    }
    std::fs::rename(&new_app, &dest)
        .with_context(|| format!("installing new {}", dest.display()))?;

    // Clean up backup.
    std::fs::remove_dir_all(&backup).ok();

    Ok(dest)
}

fn find_app_in_dir(dir: &std::path::Path) -> Option<PathBuf> {
    // Search up to 2 levels deep for a .app directory.
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "app").unwrap_or(false) && path.is_dir() {
            return Some(path);
        }
        if path.is_dir() {
            for sub in std::fs::read_dir(&path).ok()?.flatten() {
                let sub_path = sub.path();
                if sub_path.extension().map(|e| e == "app").unwrap_or(false)
                    && sub_path.is_dir()
                {
                    return Some(sub_path);
                }
            }
        }
    }
    None
}

/// Replace a single binary (Linux / Windows).
fn swap_binary(
    tmp_dir: &std::path::Path,
    current_exe: &std::path::Path,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;

    let bin_name = if cfg!(target_os = "windows") {
        "e-sh.exe"
    } else {
        "e-sh"
    };

    let new_bin = find_file_in_dir(tmp_dir, bin_name)
        .ok_or_else(|| anyhow::anyhow!("'{}' not found in downloaded archive", bin_name))?;

    if cfg!(target_os = "windows") {
        // On Windows the running executable is locked by the OS, so we cannot
        // rename or overwrite it directly.  Instead we:
        //   1. Place the new binary next to the current one with a `.new` suffix.
        //   2. Write a small batch script that waits for this process to exit,
        //      swaps the files, and relaunches the app.
        //   3. Return the path to the batch script so the caller can launch it
        //      before quitting.
        let new_dest = current_exe.with_extension("new.exe");
        if new_dest.exists() {
            std::fs::remove_file(&new_dest).ok();
        }
        std::fs::copy(&new_bin, &new_dest)
            .with_context(|| format!("copying new binary to {}", new_dest.display()))?;

        // Also update the companion e-sh-rdp.exe if present in the archive.
        let rdp_current = current_exe.with_file_name("e-sh-rdp.exe");
        let rdp_new_dest = current_exe.with_file_name("e-sh-rdp.new.exe");
        if let Some(rdp_bin) = find_file_in_dir(tmp_dir, "e-sh-rdp.exe") {
            if rdp_new_dest.exists() {
                std::fs::remove_file(&rdp_new_dest).ok();
            }
            std::fs::copy(&rdp_bin, &rdp_new_dest).ok();
        }

        let script = current_exe.with_extension("update.bat");
        let exe_path = current_exe.to_string_lossy();
        let new_path = new_dest.to_string_lossy();
        let rdp_path = rdp_current.to_string_lossy();
        let rdp_new_path = rdp_new_dest.to_string_lossy();

        // The batch script:
        //  - Waits in a loop until the old exe is no longer locked
        //  - Replaces the old exe with the new one
        //  - Optionally replaces e-sh-rdp.exe
        //  - Relaunches the app
        //  - Deletes itself
        let bat_content = format!(
            "@echo off\r\n\
             :wait\r\n\
             timeout /t 1 /nobreak >nul\r\n\
             del \"{exe_path}\" >nul 2>&1\r\n\
             if exist \"{exe_path}\" goto wait\r\n\
             move /y \"{new_path}\" \"{exe_path}\" >nul\r\n\
             if exist \"{rdp_new_path}\" (\r\n\
               del \"{rdp_path}\" >nul 2>&1\r\n\
               move /y \"{rdp_new_path}\" \"{rdp_path}\" >nul\r\n\
             )\r\n\
             start \"\" \"{exe_path}\"\r\n\
             del \"%~f0\" >nul 2>&1\r\n"
        );
        std::fs::write(&script, bat_content)
            .with_context(|| format!("writing update script {}", script.display()))?;

        Ok(script)
    } else {
        // Unix: rename is atomic and works on running binaries.
        let backup = current_exe.with_extension("bak");
        if backup.exists() {
            std::fs::remove_file(&backup).ok();
        }
        std::fs::rename(current_exe, &backup)
            .with_context(|| format!("backing up {}", current_exe.display()))?;
        std::fs::rename(&new_bin, current_exe)
            .with_context(|| format!("installing new binary to {}", current_exe.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(current_exe, std::fs::Permissions::from_mode(0o755)).ok();
        }

        std::fs::remove_file(&backup).ok();

        Ok(current_exe.to_path_buf())
    }
}

fn find_file_in_dir(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    for entry in walkdir(dir) {
        if entry.file_name().map(|n| n == name).unwrap_or(false) && entry.is_file() {
            return Some(entry);
        }
    }
    None
}

fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                result.extend(walkdir(&path));
            } else {
                result.push(path);
            }
        }
    }
    result
}
