//! Knowing when a newer version has been published, and asking the machine
//! to install it.
//!
//! The server never replaces itself: it runs without the rights to. Where it
//! was installed with `deploy/install.sh`, creating one empty file makes
//! systemd start the installer as root, which downloads the latest release
//! from the project's own page and checks it before installing.

use crate::service::{user_problem, Problem};
use crate::{alerts, db, App};
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where releases are published.
pub const REPO: &str = "Church54711/amepricetrack";

/// Where the root installer leaves a note on how the last requested update went.
const STATUS_FILE: &str = "/run/amepricetrack-update.status";

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Release {
    pub version: String,
    /// The release's page, for reading what changed.
    pub page: String,
}

/// The numbers of a version such as "v1.2.3", or None when it is not one.
fn numbers(text: &str) -> Option<Vec<u32>> {
    let text = text.trim().trim_start_matches('v');
    let text = text.split('-').next().unwrap_or(text);
    let parts: Option<Vec<u32>> = text.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 4)
}

/// True when `candidate` is a later version than `installed`.
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (numbers(candidate), numbers(installed)) else {
        return false;
    };
    let width = a.len().max(b.len());
    a.resize(width, 0);
    b.resize(width, 0);
    a > b
}

/// Reads GitHub's description of the latest release.
pub fn parse_release(body: &str) -> Option<Release> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let version = json.get("tag_name")?.as_str()?.trim_start_matches('v').to_string();
    numbers(&version)?;
    let page = json
        .get("html_url")
        .and_then(|u| u.as_str())
        .filter(|u| u.starts_with("https://github.com/"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://github.com/{REPO}/releases"));
    Some(Release { version, page })
}

/// The last answer, and when it was asked for.
static LAST: Mutex<Option<(Instant, Option<Release>)>> = Mutex::new(None);

const ASK_AGAIN_AFTER: Duration = Duration::from_secs(6 * 3600);

/// The latest published release, or None when it could not be found out.
/// The answer is kept for a few hours; `fresh` asks again now.
pub async fn latest(app: &App, fresh: bool) -> Option<Release> {
    if !app.cfg.update_check {
        return None;
    }
    if !fresh {
        if let Some((at, release)) = LAST.lock().ok().and_then(|last| last.clone()) {
            if at.elapsed() < ASK_AGAIN_AFTER {
                return release;
            }
        }
    }
    let address = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let asked = app
        .public_http
        .get(&address)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .and_then(|r| r.error_for_status());
    let release = match asked {
        Ok(response) => response.text().await.ok().and_then(|body| parse_release(&body)),
        Err(e) => {
            tracing::debug!("could not look for a newer version: {}", crate::fetch::root_cause(&e));
            None
        }
    };
    if let Ok(mut last) = LAST.lock() {
        *last = Some((Instant::now(), release.clone()));
    }
    release
}

/// The newer version, when there is one.
pub async fn newer(app: &App, fresh: bool) -> Option<Release> {
    latest(app, fresh).await.filter(|r| is_newer(&r.version, current()))
}

/// The newer version as far as is already known, without asking anyone.
/// Pages use this, so they never wait on the internet.
pub fn known_newer() -> Option<Release> {
    let last = LAST.lock().ok()?.clone();
    last.and_then(|(_, release)| release).filter(|r| is_newer(&r.version, current()))
}

/// True when this machine was set up so the website can ask for an update.
pub fn can_install(app: &App) -> bool {
    app.cfg.update_request.is_some()
}

/// Asks the machine to install the latest release. The server is restarted
/// by the installer about a minute later.
pub async fn request(app: &App) -> Result<(), Problem> {
    let Some(file) = &app.cfg.update_request else {
        return Err(user_problem("This server cannot update itself. On the server, run: sudo amepricetrack-update"));
    };
    tokio::fs::write(file, b"").await.map_err(|e| anyhow::anyhow!("could not ask for an update at {}: {e}", file.display()))?;
    tracing::info!("an update was requested from the website");
    Ok(())
}

/// How the last update asked for from the website went.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    /// "running", "ok" or "failed".
    pub state: String,
    pub message: String,
    pub at: i64,
}

fn parse_status(text: &str) -> Option<Status> {
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix('=')).map(|v| v.trim().to_string());
    let state = field("state").filter(|s| matches!(s.as_str(), "running" | "ok" | "failed"))?;
    Some(Status {
        state,
        message: field("message").unwrap_or_default(),
        at: field("at").and_then(|a| a.parse().ok()).unwrap_or(0),
    })
}

pub fn status() -> Option<Status> {
    parse_status(&std::fs::read_to_string(STATUS_FILE).ok()?)
}

/// Looks for a newer version once in a while and tells the administrators
/// about each one once.
pub async fn daily(app: &App) -> anyhow::Result<()> {
    let Some(release) = newer(app, false).await else {
        return Ok(());
    };
    if db::server_setting(&app.db, "update_told").await?.as_deref() == Some(release.version.as_str()) {
        return Ok(());
    }
    db::set_server_setting(&app.db, "update_told", &release.version).await?;
    let how = if can_install(app) {
        "Update from the Settings page, under Server.".to_string()
    } else {
        "On the server, run: sudo amepricetrack-update".to_string()
    };
    let body = format!("This server runs {}. {how}", current());
    let link = app.cfg.public_url.as_ref().map(|base| format!("{base}/settings")).unwrap_or_else(|| release.page.clone());
    for admin in db::list_users(&app.db).await?.into_iter().filter(|u| u.is_admin) {
        let title = format!("AmePriceTrack {} is available", release.version);
        alerts::notify(app, admin.id, None, "server", &title, &body, None, &link).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.2.1", "0.2"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(!is_newer("nightly", "0.2.0"));
        assert!(!is_newer("", "0.2.0"));
    }

    #[test]
    fn a_release_is_read_from_what_github_sends() {
        let body =
            r#"{"tag_name":"v0.3.1","html_url":"https://github.com/Church54711/amepricetrack/releases/tag/v0.3.1","assets":[]}"#;
        let release = parse_release(body).unwrap();
        assert_eq!(release.version, "0.3.1");
        assert!(release.page.ends_with("/v0.3.1"));
        // A page address from somewhere else is not passed on.
        let odd = parse_release(r#"{"tag_name":"v0.3.1","html_url":"https://example.com/x"}"#).unwrap();
        assert_eq!(odd.page, "https://github.com/Church54711/amepricetrack/releases");
        assert_eq!(parse_release(r#"{"message":"Not Found"}"#), None);
        assert_eq!(parse_release(r#"{"tag_name":"latest"}"#), None);
        assert_eq!(parse_release("<html>"), None);
    }

    #[test]
    fn the_installers_note_is_read() {
        let note = parse_status("state=failed\nat=1790000000\nmessage=The download does not match its checksum\n").unwrap();
        assert_eq!(note.state, "failed");
        assert_eq!(note.at, 1_790_000_000);
        assert!(note.message.contains("checksum"));
        assert_eq!(parse_status("state=surprise\n"), None);
        assert_eq!(parse_status(""), None);
    }
}
