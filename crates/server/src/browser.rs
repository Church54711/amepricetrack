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
    /// The tab a person is using by hand through the website, when one is.
    hands: Mutex<Option<Hands>>,
}

/// A tab a person is using by hand: to sign in at a store, choose their
/// local store, or get past a "prove you are human" page.
struct Hands {
    /// What the tab was opened for (a store), so asking for another starts afresh.
    label: String,
    tab_id: String,
    ws_url: String,
    last_active: Instant,
}

/// With no picture asked for and nothing pressed for this long, the person has left.
const HANDS_IDLE: Duration = Duration::from_secs(40);
/// Price checks wait this long for a person to finish before giving up.
const HANDS_WAIT: Duration = Duration::from_secs(600);

/// Something a person did in the browser they are using by hand. Positions
/// are fractions of the picture's width and height, from its top left corner.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HandsAction {
    Click {
        x: f64,
        y: f64,
    },
    /// Press and keep pressing, as some "prove you are human" pages ask.
    Hold {
        x: f64,
        y: f64,
        seconds: f64,
    },
    Text {
        text: String,
    },
    Key {
        key: String,
    },
    Scroll {
        dy: f64,
    },
    Back,
    Reload,
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
            hands: Mutex::new(None),
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

    /// True while a person is using the browser by hand. A tab they walked
    /// away from is closed.
    async fn in_hands(&self) -> bool {
        let mut hands = self.hands.lock().await;
        match hands.as_ref() {
            Some(h) if h.last_active.elapsed() < HANDS_IDLE => true,
            Some(h) => {
                let _ = self.http.get(self.endpoint(&format!("/json/close/{}", h.tab_id))).send().await;
                *hands = None;
                false
            }
            None => false,
        }
    }

    /// Gets the tab a person uses by hand ready for `label` (a store). The
    /// tab is started at `url`; one already open for the same store is left
    /// where the person was, unless `restart` asks for the start again.
    pub async fn hands_open(&self, label: &str, url: &str, restart: bool) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            self.ensure_running(&mut state).await?;
            state.last_used = Some(Instant::now());
        }
        let mut hands = self.hands.lock().await;
        let mut navigate = restart;
        if let Some(open) = hands.as_mut() {
            open.last_active = Instant::now();
            if open.label != label {
                open.label = label.to_string();
                navigate = true;
            }
        }
        if hands.is_none() {
            navigate = true;
            let tab: Value = self
                .http
                .put(self.endpoint("/json/new?about:blank"))
                .send()
                .await
                .context("could not open a browser tab")?
                .json()
                .await
                .context("unexpected answer from the browser")?;
            let ws_url = tab["webSocketDebuggerUrl"]
                .as_str()
                .ok_or_else(|| anyhow!("the browser did not offer a control channel"))?
                .to_string();
            *hands = Some(Hands {
                label: label.to_string(),
                tab_id: tab["id"].as_str().unwrap_or_default().to_string(),
                ws_url,
                last_active: Instant::now(),
            });
        }
        let ws_url = hands.as_ref().map(|h| h.ws_url.clone()).unwrap_or_default();
        drop(hands);
        if !navigate {
            return Ok(());
        }
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.context("could not connect to the browser tab")?;
        let mut id = 0u64;
        call(&mut ws, &mut id, "Page.navigate", json!({ "url": url })).await?;
        let _ = ws.close(None).await;
        Ok(())
    }

    /// The control channel of the tab in use by hand; using it counts as activity.
    async fn hands_channel(&self) -> Result<String> {
        let mut hands = self.hands.lock().await;
        let hands = hands.as_mut().ok_or_else(|| anyhow!("the browser is not open; open a store first"))?;
        hands.last_active = Instant::now();
        Ok(hands.ws_url.clone())
    }

    /// A picture of the tab in use by hand (JPEG), with the page's address.
    pub async fn hands_screen(&self) -> Result<(Vec<u8>, String)> {
        use base64::Engine;
        let ws_url = self.hands_channel().await?;
        self.state.lock().await.last_used = Some(Instant::now());
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.context("could not connect to the browser tab")?;
        let mut id = 0u64;
        let shot = call(&mut ws, &mut id, "Page.captureScreenshot", json!({ "format": "jpeg", "quality": 60 })).await?;
        let address = evaluate(&mut ws, &mut id, "location.href").await.unwrap_or(Value::Null);
        let _ = ws.close(None).await;
        let picture = base64::engine::general_purpose::STANDARD
            .decode(shot["data"].as_str().unwrap_or_default())
            .context("the browser sent an unreadable picture")?;
        Ok((picture, address.as_str().unwrap_or_default().to_string()))
    }

    /// Does what the person did, in the tab in use by hand.
    pub async fn hands_act(&self, action: &HandsAction) -> Result<()> {
        let ws_url = self.hands_channel().await?;
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.context("could not connect to the browser tab")?;
        let mut id = 0u64;
        // The page's size in its own units, to turn fractions into positions.
        let metrics = call(&mut ws, &mut id, "Page.getLayoutMetrics", json!({})).await?;
        let view = &metrics["cssVisualViewport"];
        let (width, height) = (view["clientWidth"].as_f64().unwrap_or(1366.0), view["clientHeight"].as_f64().unwrap_or(768.0));
        let at = |x: f64, y: f64| (x.clamp(0.0, 1.0) * width, y.clamp(0.0, 1.0) * height);
        let mouse = |kind: &str, x: f64, y: f64| json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 });
        match action {
            HandsAction::Click { x, y } => {
                let (x, y) = at(*x, *y);
                call(&mut ws, &mut id, "Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y })).await?;
                call(&mut ws, &mut id, "Input.dispatchMouseEvent", mouse("mousePressed", x, y)).await?;
                call(&mut ws, &mut id, "Input.dispatchMouseEvent", mouse("mouseReleased", x, y)).await?;
            }
            HandsAction::Hold { x, y, seconds } => {
                let (x, y) = at(*x, *y);
                call(&mut ws, &mut id, "Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y })).await?;
                call(&mut ws, &mut id, "Input.dispatchMouseEvent", mouse("mousePressed", x, y)).await?;
                tokio::time::sleep(Duration::from_secs_f64(seconds.clamp(0.5, 20.0))).await;
                call(&mut ws, &mut id, "Input.dispatchMouseEvent", mouse("mouseReleased", x, y)).await?;
            }
            HandsAction::Text { text } => {
                call(&mut ws, &mut id, "Input.insertText", json!({ "text": text })).await?;
            }
            HandsAction::Key { key } => {
                let (code, text) = match key.as_str() {
                    "Enter" => (13, Some("\r")),
                    "Tab" => (9, None),
                    "Backspace" => (8, None),
                    "Escape" => (27, None),
                    "ArrowUp" => (38, None),
                    "ArrowDown" => (40, None),
                    other => bail!("the key {other} cannot be sent"),
                };
                let mut down = json!({ "type": "rawKeyDown", "key": key, "code": key, "windowsVirtualKeyCode": code });
                if let Some(text) = text {
                    down["type"] = json!("keyDown");
                    down["text"] = json!(text);
                }
                call(&mut ws, &mut id, "Input.dispatchKeyEvent", down).await?;
                call(
                    &mut ws,
                    &mut id,
                    "Input.dispatchKeyEvent",
                    json!({ "type": "keyUp", "key": key, "code": key, "windowsVirtualKeyCode": code }),
                )
                .await?;
            }
            HandsAction::Scroll { dy } => {
                let wheel = json!({ "type": "mouseWheel", "x": width / 2.0, "y": height / 2.0, "deltaX": 0, "deltaY": dy.clamp(-3000.0, 3000.0) });
                call(&mut ws, &mut id, "Input.dispatchMouseEvent", wheel).await?;
            }
            HandsAction::Back => {
                evaluate(&mut ws, &mut id, "history.back()").await?;
            }
            HandsAction::Reload => {
                call(&mut ws, &mut id, "Page.reload", json!({})).await?;
            }
        }
        let _ = ws.close(None).await;
        Ok(())
    }

    /// Closes the tab in use by hand. What was signed in or chosen there stays in the browser.
    pub async fn hands_close(&self) {
        if let Some(h) = self.hands.lock().await.take() {
            let _ = self.http.get(self.endpoint(&format!("/json/close/{}", h.tab_id))).send().await;
        }
    }

    /// Opens `url` in a new tab, waits for the page to finish, and returns its HTML.
    pub async fn fetch_html(&self, url: &str) -> Result<String> {
        // A person using the browser by hand comes first: their tab has to stay in front.
        let asked = Instant::now();
        while self.in_hands().await {
            if asked.elapsed() > HANDS_WAIT {
                bail!("the server's browser is being used by a person; price checks continue when they are done");
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
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
        if self.in_hands().await {
            return;
        }
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
