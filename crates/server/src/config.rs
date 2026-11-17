//! Settings read from environment variables at startup. Everything has a
//! default, so the server runs with no configuration at all.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// Address the web server listens on. `APT_BIND`, default `0.0.0.0:8787`.
    pub bind: SocketAddr,
    /// Where the database and the browser profile live. `APT_DATA_DIR`, default `./data`.
    pub data_dir: PathBuf,
    /// Address people use to reach this server, used for links in alerts.
    /// `APT_PUBLIC_URL`, e.g. `http://192.168.1.20:8787`.
    pub public_url: Option<String>,
    /// How often each product is re-checked. `APT_CHECK_INTERVAL_MINUTES`, default 240.
    pub check_interval: Duration,
    /// Chrome or Chromium program used for stores that reject plain requests.
    /// `APT_BROWSER` (path), found automatically when unset. `APT_BROWSER=off` disables it.
    pub browser_path: Option<PathBuf>,
    /// Local port Chrome's control channel listens on. `APT_BROWSER_PORT`, default 9377.
    pub browser_port: u16,
    /// Extra command line arguments for Chrome. `APT_BROWSER_ARGS`, space separated.
    pub browser_args: Vec<String>,
    /// Target store whose prices are used. `APT_TARGET_STORE_ID`, default 1375.
    pub target_store_id: String,
    /// Send cookies only over HTTPS. `APT_SECURE_COOKIES=1` when served behind HTTPS.
    pub secure_cookies: bool,
    /// How many daily copies of the database are kept. `APT_BACKUP_KEEP`, default 14; 0 turns backups off.
    pub backup_keep: usize,
    /// Where those copies go. `APT_BACKUP_DIR`, default `backups` inside the data folder.
    pub backup_dir: PathBuf,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = env("APT_BIND").unwrap_or_else(|| "0.0.0.0:8787".into());
        let bind: SocketAddr = bind.parse().map_err(|_| anyhow::anyhow!("APT_BIND must look like 0.0.0.0:8787, got '{bind}'"))?;
        let minutes: u64 = match env("APT_CHECK_INTERVAL_MINUTES") {
            Some(v) => v.parse().map_err(|_| anyhow::anyhow!("APT_CHECK_INTERVAL_MINUTES must be a whole number, got '{v}'"))?,
            None => 240,
        };
        let browser_path = match env("APT_BROWSER") {
            Some(v) if matches!(v.to_ascii_lowercase().as_str(), "off" | "none" | "false" | "0") => None,
            Some(v) => {
                let path = PathBuf::from(&v);
                let found = if path.is_file() { Some(path) } else { find_in_path(&v) };
                if found.is_none() {
                    anyhow::bail!("APT_BROWSER is set to '{v}' but no such program was found");
                }
                found
            }
            None => find_browser(),
        };
        let browser_port = match env("APT_BROWSER_PORT") {
            Some(v) => v.parse().map_err(|_| anyhow::anyhow!("APT_BROWSER_PORT must be a port number, got '{v}'"))?,
            None => 9377,
        };
        let data_dir = PathBuf::from(env("APT_DATA_DIR").unwrap_or_else(|| "data".into()));
        let backup_keep = match env("APT_BACKUP_KEEP") {
            Some(v) => v.parse().map_err(|_| anyhow::anyhow!("APT_BACKUP_KEEP must be a whole number, got '{v}'"))?,
            None => 14,
        };
        Ok(Config {
            bind,
            backup_keep,
            backup_dir: env("APT_BACKUP_DIR").map(PathBuf::from).unwrap_or_else(|| data_dir.join("backups")),
            data_dir,
            public_url: env("APT_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string()),
            check_interval: Duration::from_secs(minutes.clamp(15, 7 * 24 * 60) * 60),
            browser_path,
            browser_port,
            browser_args: env("APT_BROWSER_ARGS").map(|a| split_args(&a)).unwrap_or_default(),
            target_store_id: env("APT_TARGET_STORE_ID").unwrap_or_else(|| "1375".into()),
            secure_cookies: env("APT_SECURE_COOKIES").is_some_and(|v| v != "0"),
        })
    }
}

/// Splits a command line on spaces, keeping "double quoted parts" together:
/// `--a=1 --name="two words"` becomes `--a=1` and `--name=two words`.
fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
        let exe = dir.join(format!("{program}.exe"));
        exe.is_file().then_some(exe)
    })
}

/// Looks for an installed Chrome or Chromium in the usual places.
fn find_browser() -> Option<PathBuf> {
    for name in ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser"] {
        if let Some(path) = find_in_path(name) {
            return Some(path);
        }
    }
    let fixed = [
        "/opt/google/chrome/chrome",
        "/snap/bin/chromium",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    fixed.iter().map(Path::new).find(|p| p.is_file()).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::split_args;

    #[test]
    fn arguments_split_on_spaces_outside_quotes() {
        assert_eq!(split_args("--a=1   --b"), vec!["--a=1", "--b"]);
        assert_eq!(split_args(r#"--user-agent="Some Browser 1.0" --c"#), vec!["--user-agent=Some Browser 1.0", "--c"]);
        assert!(split_args("   ").is_empty());
    }
}
