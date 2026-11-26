//! Updating lclip from its GitHub releases.
//!
//! The daemon asks GitHub for the latest release now and then. When the window's update
//! button is pressed it downloads the tarball for this OS, checks it against the release's
//! sha256 file, swaps the binary in place and execs the new one. `curl`, `tar` and a sha256
//! tool do the work, so lclip carries no HTTP or archive code of its own.

use std::{
    env, fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{self, Command},
    sync::{Arc, OnceLock},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    ctl::{Action, Ctl},
    store::Shared,
};

const LATEST: &str = "https://api.github.com/repos/thesarfo/lclip/releases/latest";
const EVERY: Duration = Duration::from_secs(6 * 3600);

/// What the window shows about updates.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Update {
    #[default]
    None,
    Available { version: String },
    Installing { version: String },
    Failed { version: String, error: String },
}

/// The release tarball built for this machine, as named by the release workflow.
fn asset_suffix() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("universal-macos.tar.gz")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-linux.tar.gz")
    } else {
        None
    }
}

/// Where this binary lives. Read once at start: on Linux `current_exe` turns into
/// "... (deleted)" after the file is replaced.
pub fn exe() -> &'static Path {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| env::current_exe().unwrap_or_else(|_| PathBuf::from("lclip")))
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn curl(url: &str, out: Option<&Path>) -> Result<Vec<u8>> {
    let mut cmd = Command::new("curl");
    cmd.args(["-fsSL", "--proto", "=https", "--max-time", "300"]);
    if url.starts_with("https://api.github.com/") {
        cmd.args(["-H", "Accept: application/vnd.github+json"]);
    }
    cmd.args(["-A", concat!("lclip/", env!("CARGO_PKG_VERSION"))]);
    if let Some(out) = out {
        cmd.arg("-o").arg(out);
    }
    let res = cmd.arg(url).output().context("curl is needed to update")?;
    ensure!(res.status.success(), "download failed: {}", String::from_utf8_lossy(&res.stderr).trim());
    Ok(res.stdout)
}

fn version(tag: &str) -> Vec<u64> {
    tag.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// The newest release, if it is newer than this build and has a tarball for this machine.
fn newer() -> Result<Option<Release>> {
    let rel: Release = serde_json::from_slice(&curl(LATEST, None)?)?;
    let ours = version(env!("CARGO_PKG_VERSION"));
    let has_asset = asset_suffix().is_some_and(|s| rel.assets.iter().any(|a| a.name.ends_with(s)));
    Ok((version(&rel.tag_name) > ours && has_asset).then_some(rel))
}

fn sha256(file: &Path) -> Result<String> {
    let out = if cfg!(target_os = "macos") {
        Command::new("shasum").args(["-a", "256"]).arg(file).output()
    } else {
        Command::new("sha256sum").arg(file).output()
    }
    .context("no sha256 tool to check the download")?;
    ensure!(out.status.success(), "could not check the download");
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.split_whitespace().next().unwrap_or_default().to_lowercase())
}

/// Download, check and put the new binary where this one is.
fn install(rel: &Release) -> Result<()> {
    let suffix = asset_suffix().context("no release is built for this machine")?;
    let tarball = rel.assets.iter().find(|a| a.name.ends_with(suffix)).context("release has no build for this machine")?;
    let sum = rel
        .assets
        .iter()
        .find(|a| a.name == format!("{}.sha256", tarball.name))
        .context("release has no checksum")?;

    let work = env::temp_dir().join(format!("lclip-update-{}", process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = (|| {
        let tar = work.join(&tarball.name);
        curl(&tarball.browser_download_url, Some(&tar))?;
        let want = String::from_utf8_lossy(&curl(&sum.browser_download_url, None)?)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase();
        ensure!(!want.is_empty() && sha256(&tar)? == want, "the download did not match its checksum");

        let status = Command::new("tar").arg("xzf").arg(&tar).arg("-C").arg(&work).status()?;
        ensure!(status.success(), "could not unpack the download");
        let dir = tarball.name.trim_end_matches(".tar.gz");
        let new = work.join(dir).join("lclip");

        // Make sure it runs here and is the version we asked for before trusting it.
        let out = Command::new(&new).arg("--version").output().context("the new build does not run")?;
        let said = String::from_utf8_lossy(&out.stdout);
        if version(said.trim().trim_start_matches("lclip ")) != version(&rel.tag_name) {
            bail!("the new build reports {}", said.trim());
        }

        // Copy next to the old binary, then rename over it so there is never a half-written lclip.
        let target = exe();
        let staged = target.with_file_name(".lclip-update");
        fs::copy(&new, &staged).with_context(|| format!("cannot write to {}", target.display()))?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        fs::rename(&staged, target).inspect_err(|_| {
            let _ = fs::remove_file(&staged);
        })?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

/// Check for updates in the background while the setting is on.
pub fn spawn_checker(shared: Shared) {
    exe();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(10));
        loop {
            if shared.lock().unwrap().settings.check_updates {
                match newer() {
                    Ok(Some(rel)) => {
                        let mut s = shared.lock().unwrap();
                        if matches!(s.update, Update::None | Update::Available { .. }) {
                            s.update = Update::Available { version: rel.tag_name.trim_start_matches('v').into() };
                        }
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("lclip: update check: {e:#}"),
                }
            }
            thread::sleep(EVERY);
        }
    });
}

/// The window's update button. Restarts lclip on the new version when it worked.
pub fn start(shared: Shared, ctl: Arc<Ctl>) {
    {
        let mut s = shared.lock().unwrap();
        let version = match &s.update {
            Update::Available { version } | Update::Failed { version, .. } => version.clone(),
            _ => return,
        };
        s.update = Update::Installing { version };
    }
    thread::spawn(move || {
        let res = newer().and_then(|rel| match rel {
            Some(rel) => install(&rel),
            None => bail!("no newer release found"),
        });
        match res {
            Ok(()) => ctl.send(Action::Restart),
            Err(e) => {
                eprintln!("lclip: update: {e:#}");
                let mut s = shared.lock().unwrap();
                if let Update::Installing { version } = &s.update {
                    s.update = Update::Failed { version: version.clone(), error: format!("{e:#}") };
                }
            }
        }
    });
}

/// Replace this process with the binary now on disk. Only returns on failure.
pub fn relaunch() -> std::io::Error {
    Command::new(exe()).arg("--daemon").exec()
}
