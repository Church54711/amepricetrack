//! Reads a product page through a real Chrome window.
//!
//! Several stores answer plain requests and headless browsers with an
//! "access denied" page, but serve an ordinary Chrome normally. So the server
//! keeps one ordinary Chrome running (on a virtual screen when the machine
//! has no display) and asks it for the finished page over Chrome's local
//! control channel (the DevTools protocol).

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct Browser {
    exe: PathBuf,
    profile_dir: PathBuf,
    port: u16,
    extra_args: Vec<String>,
    http: reqwest::Client,
    /// Held for a whole page load, so only one page is open at a time.
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    chrome: Option<Child>,
    xvfb: Option<Child>,
    last_used: Option<Instant>,
}

/// Longest a single page may take from navigation to finished HTML.
const PAGE_TIMEOUT: Duration = Duration::from_secs(45);

impl Browser {
    pub fn new(exe: PathBuf, profile_dir: PathBuf, port: u16, extra_args: Vec<String>) -> Self {
        Browser {
            exe,
            profile_dir,
            port,
            extra_args,
            http: reqwest::Client::builder().no_proxy().build().expect("local http client"),
            state: Mutex::new(State::default()),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    async fn is_up(&self) -> bool {
        matches!(
            self.http.get(self.endpoint("/json/version")).timeout(Duration::from_secs(2)).send().await,
            Ok(r) if r.status().is_success()
        )
    }

    async fn ensure_running(&self, state: &mut State) -> Result<()> {
        if self.is_up().await {
            return Ok(());
        }
        // A Chrome we started earlier may have died; clear it before starting again.
        if let Some(mut old) = state.chrome.take() {
            let _ = old.kill().await;
        }
        tokio::fs::create_dir_all(&self.profile_dir).await.ok();

        let mut cmd = Command::new(&self.exe);
        cmd.arg(format!("--remote-debugging-port={}", self.port))
            .arg(format!("--user-data-dir={}", self.profile_dir.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--password-store=basic",
                "--disable-session-crashed-bubble",
                "--window-size=1366,900",
            ])
            .args(&self.extra_args)
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        // A server normally has no screen. Chrome still needs one, so give it a virtual one.
        if cfg!(all(unix, not(target_os = "macos")))
            && std::env::var_os("DISPLAY").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
        {
            let display = std::env::var("APT_XVFB_DISPLAY").unwrap_or_else(|_| ":99".into());
            if state.xvfb.is_none() {
                let xvfb = Command::new("Xvfb")
                    .args([display.as_str(), "-screen", "0", "1366x900x24", "-nolisten", "tcp"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .context("this machine has no screen and Xvfb is not installed (sudo apt install xvfb)")?;
                state.xvfb = Some(xvfb);
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
            cmd.env("DISPLAY", &display);
        }

        let child = cmd.spawn().with_context(|| format!("could not start the browser at {}", self.exe.display()))?;
        state.chrome = Some(child);

        for _ in 0..60 {
            if self.is_up().await {
                tracing::info!("browser started");
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if let Some(mut c) = state.chrome.take() {
            let _ = c.kill().await;
        }
        bail!("the browser was started but never became ready")
    }

    /// Opens `url` in a new tab, waits for the page to finish, and returns its HTML.
    pub async fn fetch_html(&self, url: &str) -> Result<String> {
        let mut state = self.state.lock().await;
        self.ensure_running(&mut state).await?;
        state.last_used = Some(Instant::now());

        let tab: Value = self
            .http
            .put(self.endpoint("/json/new?about:blank"))
            .send()
            .await
            .context("could not open a browser tab")?
            .json()
            .await
            .context("unexpected answer from the browser")?;
        let tab_id = tab["id"].as_str().unwrap_or_default().to_string();
        let ws_url = tab["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| anyhow!("the browser did not offer a control channel"))?
            .to_string();

        let result = tokio::time::timeout(PAGE_TIMEOUT, load_page(&ws_url, url)).await;

        // Always close the tab, even when loading failed.
        let _ = self.http.get(self.endpoint(&format!("/json/close/{tab_id}"))).send().await;
        state.last_used = Some(Instant::now());

        match result {
            Ok(inner) => inner,
            Err(_) => bail!("the page took longer than {} seconds to load", PAGE_TIMEOUT.as_secs()),
        }
    }

    /// Closes Chrome when it has not been needed for a while, to free memory.
    pub async fn shutdown_if_idle(&self, idle: Duration) {
        let mut state = self.state.lock().await;
        if state.chrome.is_some() && state.last_used.is_some_and(|t| t.elapsed() > idle) {
            Self::stop(&mut state).await;
            tracing::info!("browser closed after being idle");
        }
    }

    pub async fn shutdown(&self) {
        let mut state = self.state.lock().await;
        Self::stop(&mut state).await;
    }

    async fn stop(state: &mut State) {
        if let Some(mut chrome) = state.chrome.take() {
            let _ = chrome.kill().await;
        }
        if let Some(mut xvfb) = state.xvfb.take() {
            let _ = xvfb.kill().await;
        }
        state.last_used = None;
    }
}

/// One DevTools request: send it, then read until the matching answer arrives.
async fn call(ws: &mut Socket, next_id: &mut u64, method: &str, params: Value) -> Result<Value> {
    *next_id += 1;
    let id = *next_id;
    ws.send(Message::text(json!({ "id": id, "method": method, "params": params }).to_string()))
        .await
        .context("lost the connection to the browser")?;
    while let Some(msg) = ws.next().await {
        let msg = msg.context("lost the connection to the browser")?;
        let Message::Text(text) = msg else { continue };
        let value: Value = match serde_json::from_str(text.as_str()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value["id"].as_u64() == Some(id) {
            if let Some(err) = value.get("error") {
                bail!("browser error during {method}: {}", err["message"].as_str().unwrap_or("unknown"));
            }
            return Ok(value["result"].clone());
        }
    }
    bail!("the browser closed the connection")
}

async fn evaluate(ws: &mut Socket, next_id: &mut u64, expression: &str) -> Result<Value> {
    let result = call(ws, next_id, "Runtime.evaluate", json!({ "expression": expression, "returnByValue": true })).await?;
    Ok(result["result"]["value"].clone())
}

async fn load_page(ws_url: &str, url: &str) -> Result<String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await.context("could not connect to the browser tab")?;
    let mut id = 0u64;
    call(&mut ws, &mut id, "Page.enable", json!({})).await?;
    call(&mut ws, &mut id, "Page.navigate", json!({ "url": url })).await?;

    // Store pages keep filling in after "load". Wait until the page is complete
    // and its product data has appeared, or a sensible time has passed.
    const PROBE: &str = r#"JSON.stringify({
        ready: document.readyState,
        data: document.querySelectorAll('script[type="application/ld+json"], script#__NEXT_DATA__').length,
        size: document.documentElement.outerHTML.length
    })"#;
    let started = Instant::now();
    let mut last_size = 0u64;
    let mut stable_rounds = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let waited = started.elapsed();
        let probe = evaluate(&mut ws, &mut id, PROBE).await.unwrap_or(Value::Null);
        let probe: Value = probe.as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
        let complete = probe["ready"].as_str() == Some("complete");
        let has_data = probe["data"].as_u64().unwrap_or(0) > 0;
        let size = probe["size"].as_u64().unwrap_or(0);
        stable_rounds = if size == last_size && size > 0 { stable_rounds + 1 } else { 0 };
        last_size = size;

        let settled = complete && stable_rounds >= 2;
        if waited >= Duration::from_secs(4) && complete && has_data && stable_rounds >= 1 {
            break;
        }
        if waited >= Duration::from_secs(12) && settled {
            break;
        }
        if waited >= Duration::from_secs(25) {
            break;
        }
    }

    let html = evaluate(&mut ws, &mut id, "document.documentElement.outerHTML").await?;
    let _ = ws.close(None).await;
    html.as_str().map(str::to_string).filter(|h| !h.is_empty()).ok_or_else(|| anyhow!("the browser returned an empty page"))
}
