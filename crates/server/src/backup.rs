//! Daily copies of the database.
//!
//! Everything the server knows lives in one SQLite file. Once a day a
//! complete copy is written to the backup folder, and copies beyond the
//! newest few are removed. The copy is made by SQLite itself, so it is
//! consistent even while prices are being recorded.

use crate::db::{self, Db};
use crate::{alerts, App};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const PREFIX: &str = "amepricetrack-";
const SUFFIX: &str = ".db";
/// Set while backups are failing, so the administrator is told once, not every hour.
const FAILED_KEY: &str = "backup_failed";

/// One copy of the database in the backup folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub path: PathBuf,
    /// The day it was made, as 2026-11-17 (UTC).
    pub day: String,
    pub bytes: u64,
    /// When the file was last written, in seconds.
    pub made_at: Option<i64>,
}

fn today() -> String {
    time::OffsetDateTime::now_utc().date().to_string()
}

fn file_name(day: &str) -> String {
    format!("{PREFIX}{day}{SUFFIX}")
}

/// The day in a backup's file name, when the name is one of ours.
fn day_of(name: &str) -> Option<&str> {
    let day = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let shaped =
        day.len() == 10 && day.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    shaped.then_some(day)
}

/// The backups in a folder, newest first. Other files there are left alone.
pub fn list(dir: &Path) -> Vec<Backup> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut found: Vec<Backup> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let day = day_of(&name)?.to_string();
            let meta = entry.metadata().ok()?;
            let made_at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_secs() as i64);
            Some(Backup { path: entry.path(), day, bytes: meta.len(), made_at })
        })
        .collect();
    found.sort_by(|a, b| b.day.cmp(&a.day));
    found
}

/// One copy is written at a time: the daily one and "Back up now" share a file name.
static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Writes a copy of the database into `dir`, replacing today's if there is one.
pub async fn write(db: &Db, dir: &Path) -> Result<Backup> {
    let _one_at_a_time = WRITING.lock().await;
    std::fs::create_dir_all(dir).with_context(|| format!("could not create the backup folder {}", dir.display()))?;
    let day = today();
    let path = dir.join(file_name(&day));
    // Written under another name first, so a half-written copy is never taken for a backup.
    let partial = dir.join(format!("{}.partial", file_name(&day)));
    let _ = std::fs::remove_file(&partial);
    // The copy holds sign-ins and keys. The file is made first, empty and readable by the
    // server's own account only, and SQLite then fills it.
    create_private(&partial).with_context(|| format!("could not create a file in {}", dir.display()))?;
    // SQLite takes the file name as text in the statement itself; a quote in it is doubled.
    let target = partial.to_string_lossy().replace('\'', "''");
    sqlx::query(sqlx::AssertSqlSafe(format!("VACUUM INTO '{target}'")))
        .execute(db)
        .await
        .with_context(|| format!("could not write a copy of the database to {}", dir.display()))?;
    // SQLite quietly writes nothing for a database that only lives in memory.
    if std::fs::metadata(&partial).map(|m| m.len()).unwrap_or(0) == 0 {
        let _ = std::fs::remove_file(&partial);
        anyhow::bail!("the database gave no copy to write to {}", dir.display());
    }
    std::fs::rename(&partial, &path).with_context(|| format!("could not put the backup in place at {}", path.display()))?;
    list(dir).into_iter().find(|b| b.day == day).context("the backup was written but cannot be found")
}

/// Creates an empty file that, on Unix, only the server's own account may read.
fn create_private(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map(|_| ())
}

/// Removes all but the newest `keep` backups. Returns how many were removed.
pub fn prune(dir: &Path, keep: usize) -> usize {
    let mut removed = 0;
    for old in list(dir).into_iter().skip(keep.max(1)) {
        match std::fs::remove_file(&old.path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!("could not remove the old backup {}: {e}", old.path.display()),
        }
    }
    removed
}

/// Called every hour: makes today's backup when there is none yet.
pub async fn daily(app: &App) -> Result<()> {
    let keep = app.cfg.backup_keep;
    if keep == 0 {
        return Ok(());
    }
    let dir = &app.cfg.backup_dir;
    if list(dir).first().is_some_and(|newest| newest.day == today()) {
        return Ok(());
    }
    match write(&app.db, dir).await {
        Ok(backup) => {
            let removed = prune(dir, keep);
            tracing::info!("backup written to {} ({} bytes); {removed} old ones removed", backup.path.display(), backup.bytes);
            if db::server_setting(&app.db, FAILED_KEY).await?.is_some() {
                db::set_server_setting(&app.db, FAILED_KEY, "").await?;
            }
        }
        Err(e) => {
            tracing::error!("the daily backup failed: {e:#}");
            // Say so once; a backup nobody knows is missing is no backup.
            if db::server_setting(&app.db, FAILED_KEY).await?.is_none() {
                db::set_server_setting(&app.db, FAILED_KEY, &db::now().to_string()).await?;
                let link = app.cfg.public_url.as_ref().map(|base| format!("{base}/settings")).unwrap_or_default();
                let body = format!(
                    "The server could not write its daily copy of the database to {}: {e:#}. \
                     Until this is fixed there is no fresh backup of your prices and settings.",
                    dir.display()
                );
                for admin in db::list_users(&app.db).await?.into_iter().filter(|u| u.is_admin) {
                    alerts::notify(app, admin.id, None, "server", "The daily backup failed", &body, None, &link).await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty folder of its own under the system's temporary folder.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apt-backup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_our_own_files_count_as_backups() {
        assert_eq!(day_of("amepricetrack-2026-11-17.db"), Some("2026-11-17"));
        for other in
            ["amepricetrack.db", "amepricetrack-2026-11-17.db.partial", "amepricetrack-yesterday.db", "notes-2026-11-17.db"]
        {
            assert_eq!(day_of(other), None, "{other}");
        }
    }

    #[test]
    fn old_backups_are_removed_and_other_files_left_alone() {
        let dir = scratch("prune");
        for day in ["2026-11-10", "2026-11-11", "2026-11-12", "2026-11-13"] {
            std::fs::write(dir.join(file_name(day)), b"copy").unwrap();
        }
        std::fs::write(dir.join("my-notes.txt"), b"keep me").unwrap();
        assert_eq!(list(&dir).iter().map(|b| b.day.as_str()).collect::<Vec<_>>()[0], "2026-11-13", "newest first");
        assert_eq!(prune(&dir, 2), 2);
        assert_eq!(list(&dir).iter().map(|b| b.day.as_str()).collect::<Vec<_>>(), vec!["2026-11-13", "2026-11-12"]);
        assert!(dir.join("my-notes.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_backup_is_a_complete_database_that_opens_by_itself() {
        // A database in a file, as the running server has.
        let data = scratch("data");
        let live = db::open(&data).await.unwrap();
        let user = db::create_user(&live, "sam", "h", true).await.unwrap();
        let chair = crate::retailers::identify("https://www.staples.com/chair/product_555").unwrap();
        let product = db::upsert_product(&live, &chair).await.unwrap();
        db::add_watch(&live, user, product, Some(4500)).await.unwrap();

        let dir = scratch("write");
        let backup = write(&live, &dir).await.unwrap();
        assert_eq!(backup.day, today());
        assert!(backup.bytes > 0);
        // Writing again the same day replaces the copy instead of failing or piling up.
        write(&live, &dir).await.unwrap();
        assert_eq!(list(&dir).len(), 1);
        assert!(!dir.join(format!("{}.partial", file_name(&today()))).exists());

        // The copy is a database of its own with the same contents.
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&backup.path).read_only(true);
        let copy = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(options).await.unwrap();
        let tracked: (String, i64) =
            sqlx::query_as("SELECT p.url, w.target_price_cents FROM watches w JOIN products p ON p.id = w.product_id")
                .fetch_one(&copy)
                .await
                .unwrap();
        assert_eq!(tracked, ("https://www.staples.com/chair/product_555".to_string(), 4500));
        copy.close().await;
        live.close().await;
        std::fs::remove_dir_all(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&data);
    }

    #[tokio::test]
    async fn a_failing_backup_is_announced_to_the_administrator_once() {
        // The backup "folder" is a file, so nothing can be written into it.
        let blocker = scratch("fail").join("not-a-folder");
        std::fs::write(&blocker, b"in the way").unwrap();
        let base = crate::test_app().await;
        let cfg = crate::config::Config { backup_keep: 3, backup_dir: blocker.clone(), ..base.cfg.clone() };
        let app: App = std::sync::Arc::new(crate::AppState {
            db: db::open_in_memory().await,
            fetcher: crate::fetch::Fetcher::new(&cfg).unwrap(),
            cfg,
            http: reqwest::Client::new(),
            public_http: reqwest::Client::new(),
            wake: tokio::sync::Notify::new(),
            compare_wake: tokio::sync::Notify::new(),
        });
        let admin = db::create_user(&app.db, "sam", "h", true).await.unwrap();
        let member = db::create_user(&app.db, "alex", "h", false).await.unwrap();

        daily(&app).await.unwrap();
        daily(&app).await.unwrap();
        let told = db::notifications(&app.db, admin, 0, 10).await.unwrap();
        assert_eq!(told.len(), 1, "once, not every hour");
        assert_eq!(told[0].title, "The daily backup failed");
        assert!(db::notifications(&app.db, member, 0, 10).await.unwrap().is_empty());
        std::fs::remove_dir_all(blocker.parent().unwrap()).unwrap();
    }
}
