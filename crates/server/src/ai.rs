//! Talks to whichever AI service the person has connected.
//!
//! Two wire formats cover everything: Anthropic's Messages API for Claude,
//! and the OpenAI chat format, which OpenAI, DeepSeek, Gemini, OpenRouter,
//! Ollama and LM Studio all accept.

use crate::db::Settings;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Anthropic,
    OpenAi,
}

pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub base_url: &'static str,
    pub needs_key: bool,
    /// Suggested model. Blank where names change too often to hard-code;
    /// "Load models" on the settings page lists what the service offers.
    pub default_model: &'static str,
    /// A small, cheap model that is enough for matching products across stores.
    pub match_model: &'static str,
}

pub static PROVIDERS: &[Provider] = &[
    Provider {
        id: "claude",
        label: "Claude (Anthropic)",
        kind: Kind::Anthropic,
        base_url: "https://api.anthropic.com",
        needs_key: true,
        default_model: "claude-opus-5-5",
        match_model: "claude-haiku-5-5",
    },
    Provider {
        id: "openai",
        label: "OpenAI",
        kind: Kind::OpenAi,
        base_url: "https://api.openai.com/v1",
        needs_key: true,
        default_model: "",
        match_model: "",
    },
    Provider {
        id: "deepseek",
        label: "DeepSeek",
        kind: Kind::OpenAi,
        base_url: "https://api.deepseek.com/v1",
        needs_key: true,
        default_model: "deepseek-chat",
        match_model: "",
    },
    Provider {
        id: "gemini",
        label: "Google Gemini",
        kind: Kind::OpenAi,
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        needs_key: true,
        default_model: "",
        match_model: "",
    },
    Provider {
        id: "openrouter",
        label: "OpenRouter",
        kind: Kind::OpenAi,
        base_url: "https://openrouter.ai/api/v1",
        needs_key: true,
        default_model: "",
        match_model: "",
    },
    Provider {
        id: "ollama",
        label: "Ollama (local)",
        kind: Kind::OpenAi,
        base_url: "http://localhost:11434/v1",
        needs_key: false,
        default_model: "",
        match_model: "",
    },
    Provider {
        id: "lmstudio",
        label: "LM Studio (local)",
        kind: Kind::OpenAi,
        base_url: "http://localhost:1234/v1",
        needs_key: false,
        default_model: "",
        match_model: "",
    },
    Provider {
        id: "custom",
        label: "Other (OpenAI-compatible)",
        kind: Kind::OpenAi,
        base_url: "",
        needs_key: false,
        default_model: "",
        match_model: "",
    },
];

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

#[derive(Debug, Clone)]
pub struct AiConfig {
    pub kind: Kind,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl AiConfig {
    /// Builds the connection details from saved settings. `need_model` is
    /// false when only listing models.
    pub fn from_settings(s: &Settings, need_model: bool) -> Result<Self> {
        let provider = provider(&s.ai_provider)
            .ok_or_else(|| anyhow!("No AI service is connected yet. Choose one on the Settings page."))?;
        let base_url = if s.ai_base_url.trim().is_empty() { provider.base_url } else { s.ai_base_url.trim() };
        if base_url.is_empty() {
            bail!("The AI service address is missing. Add it on the Settings page.");
        }
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            bail!("The AI service address must start with http:// or https://");
        }
        if provider.needs_key && s.ai_api_key.trim().is_empty() {
            bail!("{} needs an API key. Add it on the Settings page.", provider.label);
        }
        if need_model && s.ai_model.trim().is_empty() {
            bail!("No AI model is chosen yet. Pick one on the Settings page.");
        }
        Ok(AiConfig {
            kind: provider.kind,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: s.ai_api_key.trim().to_string(),
            model: s.ai_model.trim().to_string(),
        })
    }

    /// The connection used for matching products across stores: the same
    /// service, with the matching model when one is set.
    pub fn for_matching(s: &Settings) -> Result<Self> {
        let mut cfg = Self::from_settings(s, true)?;
        if !s.ai_match_model.trim().is_empty() {
            cfg.model = s.ai_match_model.trim().to_string();
        }
        Ok(cfg)
    }
}

/// How a request differs from an ordinary chat turn.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The answer must be JSON of this shape (a JSON Schema). Services that
    /// cannot enforce a shape are simply asked for JSON, so the prompt must
    /// describe it too.
    pub json_schema: Option<Value>,
    /// A short factual task: spend little effort and answer briefly.
    pub quick: bool,
}

/// A refusal from the service itself (bad key, unknown model, bad request).
#[derive(Debug)]
struct ApiError {
    status: u16,
    message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// One turn of a conversation: ("user" | "assistant", text).
pub type Turn = (String, String);

/// Local models can take a while to load and answer.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Sends the conversation and returns the assistant's answer.
pub async fn complete(http: &reqwest::Client, cfg: &AiConfig, system: &str, turns: &[Turn]) -> Result<String> {
    complete_with(http, cfg, system, turns, &Options::default()).await
}

/// Like [`complete`], with the answer's shape or effort chosen by `options`.
pub async fn complete_with(
    http: &reqwest::Client,
    cfg: &AiConfig,
    system: &str,
    turns: &[Turn],
    options: &Options,
) -> Result<String> {
    let extras = options.json_schema.is_some() || options.quick;
    match request(http, cfg, system, turns, options, true).await {
        // Older models and some services reject the extra fields. The prompt
        // still asks for the same thing, so ask again without them.
        Err(e) if extras && e.downcast_ref::<ApiError>().is_some_and(|api| api.status == 400) => {
            tracing::debug!("AI service rejected the request options, retrying without them: {e}");
            request(http, cfg, system, turns, options, false).await
        }
        other => other,
    }
}

async fn request(
    http: &reqwest::Client,
    cfg: &AiConfig,
    system: &str,
    turns: &[Turn],
    options: &Options,
    with_extras: bool,
) -> Result<String> {
    let messages: Vec<Value> = turns.iter().map(|(role, content)| json!({ "role": role, "content": content })).collect();
    let request = match cfg.kind {
        Kind::Anthropic => {
            // Current Claude models think before answering, and that thinking
            // counts towards max_tokens, so the limit leaves room for both.
            let max_tokens = if options.quick { 4096 } else { 16000 };
            let mut body = json!({ "model": cfg.model, "max_tokens": max_tokens, "system": system, "messages": messages });
            if with_extras {
                let mut output = serde_json::Map::new();
                if options.quick {
                    output.insert("effort".into(), json!("low"));
                }
                if let Some(schema) = &options.json_schema {
                    output.insert("format".into(), json!({ "type": "json_schema", "schema": schema }));
                }
                if !output.is_empty() {
                    body["output_config"] = Value::Object(output);
                }
            }
            http.post(format!("{}/v1/messages", cfg.base_url))
                .header("x-api-key", &cfg.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
        }
        Kind::OpenAi => {
            let mut all = vec![json!({ "role": "system", "content": system })];
            all.extend(messages);
            let mut body = json!({ "model": cfg.model, "messages": all, "stream": false });
            if with_extras && options.json_schema.is_some() {
                body["response_format"] = json!({ "type": "json_object" });
            }
            let mut req = http.post(format!("{}/chat/completions", cfg.base_url)).json(&body);
            if !cfg.api_key.is_empty() {
                req = req.bearer_auth(&cfg.api_key);
            }
            req
        }
    };
    let body = send(request).await?;
    let (answer, stopped) = match cfg.kind {
        Kind::Anthropic => (
            body["content"]
                .as_array()
                .map(|blocks| {
                    blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("")
                })
                .unwrap_or_default(),
            body["stop_reason"].as_str().unwrap_or_default(),
        ),
        Kind::OpenAi => (
            body["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string(),
            body["choices"][0]["finish_reason"].as_str().unwrap_or_default(),
        ),
    };
    let answer = strip_thinking(&answer);
    if stopped == "refusal" {
        bail!("The AI service declined to answer this.");
    }
    if answer.is_empty() {
        if stopped == "max_tokens" || stopped == "length" {
            bail!("The AI ran out of room before it finished answering.");
        }
        bail!("The AI service answered, but with no text.");
    }
    Ok(answer)
}

/// Lists the model names the connected service offers.
pub async fn list_models(http: &reqwest::Client, cfg: &AiConfig) -> Result<Vec<String>> {
    let request = match cfg.kind {
        Kind::Anthropic => http
            .get(format!("{}/v1/models?limit=100", cfg.base_url))
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", "2023-06-01"),
        Kind::OpenAi => {
            let mut req = http.get(format!("{}/models", cfg.base_url));
            if !cfg.api_key.is_empty() {
                req = req.bearer_auth(&cfg.api_key);
            }
            req
        }
    };
    let body = send(request.timeout(Duration::from_secs(20))).await?;
    let mut models: Vec<String> = body["data"]
        .as_array()
        .map(|list| list.iter().filter_map(|m| m["id"].as_str()).map(|id| id.trim_start_matches("models/").to_string()).collect())
        .unwrap_or_default();
    models.sort();
    models.dedup();
    if models.is_empty() {
        bail!("The AI service did not list any models.");
    }
    Ok(models)
}

async fn send(request: reqwest::RequestBuilder) -> Result<Value> {
    let (client, request) = request.build_split();
    let mut request = request?;
    let timeout = request.timeout().copied().unwrap_or(TIMEOUT);
    *request.timeout_mut() = Some(timeout);
    let host = request.url().host_str().unwrap_or("the AI service").to_string();
    let response = match client.execute(request).await {
        Err(e) if e.is_timeout() => bail!("{host} did not answer within {} seconds.", timeout.as_secs()),
        Err(e) if e.is_connect() => bail!("Could not connect to {host}. Is it running and is the address right?"),
        Err(e) => bail!("Could not reach {host}: {}", crate::fetch::root_cause(&e)),
        Ok(r) => r,
    };
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        // Both formats put the reason in error.message; some send a bare string.
        let reason = parsed["error"]["message"]
            .as_str()
            .or(parsed["error"].as_str())
            .or(parsed["message"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| text.chars().take(200).collect());
        let hint = match status.as_u16() {
            401 | 403 => " Check the API key.",
            404 => " Check the model name and the service address.",
            429 => " The service is rate limiting or the account is out of credit.",
            _ => "",
        };
        let message = format!("{host} answered {}: {}.{hint}", status.as_u16(), reason.trim().trim_end_matches('.'));
        return Err(ApiError { status: status.as_u16(), message }.into());
    }
    if parsed.is_null() {
        bail!("{host} sent an answer that is not valid JSON. Check the service address.");
    }
    Ok(parsed)
}

/// Some local reasoning models put their scratch work in <think> tags before the answer.
fn strip_thinking(answer: &str) -> String {
    let mut text = answer.trim();
    if let Some(start) = text.find("<think>") {
        if start == 0 {
            if let Some(end) = text.find("</think>") {
                text = text[end + "</think>".len()..].trim();
            }
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(provider: &str, key: &str, model: &str, base: &str) -> Settings {
        Settings {
            ai_provider: provider.into(),
            ai_api_key: key.into(),
            ai_model: model.into(),
            ai_base_url: base.into(),
            ..Default::default()
        }
    }

    #[test]
    fn config_uses_provider_defaults_and_validates() {
        let c = AiConfig::from_settings(&settings("claude", "sk-x", "claude-opus-5-5", ""), true).unwrap();
        assert_eq!(c.kind, Kind::Anthropic);
        assert_eq!(c.base_url, "https://api.anthropic.com");

        let c = AiConfig::from_settings(&settings("ollama", "", "llama3", "http://nas:11434/v1/"), true).unwrap();
        assert_eq!(c.base_url, "http://nas:11434/v1");
        assert_eq!(c.kind, Kind::OpenAi);

        assert!(AiConfig::from_settings(&settings("", "", "", ""), true).is_err());
        assert!(AiConfig::from_settings(&settings("openai", "", "x", ""), true).is_err(), "key required");
        assert!(AiConfig::from_settings(&settings("openai", "k", "", ""), true).is_err(), "model required");
        assert!(AiConfig::from_settings(&settings("openai", "k", "", ""), false).is_ok(), "listing needs no model");
        assert!(AiConfig::from_settings(&settings("custom", "", "m", ""), true).is_err(), "address required");
        assert!(AiConfig::from_settings(&settings("custom", "", "m", "ftp://x"), true).is_err());
    }

    #[test]
    fn thinking_is_removed() {
        assert_eq!(strip_thinking("<think>hmm</think>\n\nThe answer."), "The answer.");
        assert_eq!(strip_thinking("Plain answer."), "Plain answer.");
    }
}

#[cfg(test)]
#[path = "tests_ai.rs"]
mod wire_tests;
