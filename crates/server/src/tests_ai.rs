//! Checks the two AI wire formats against a stand-in service on localhost.

use super::*;
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(String, axum::http::HeaderMap, Value)>>>;

/// Starts a stand-in AI service that records what it was sent and answers
/// with `reply`. Returns its address and the recorder.
async fn stand_in(status: u16, reply: Value) -> (String, Seen) {
    use axum::extract::Request;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let handler = move |request: Request| {
        let recorder = recorder.clone();
        let reply = reply.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            recorder.lock().unwrap().push((parts.uri.to_string(), parts.headers, json));
            (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(reply))
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, axum::Router::new().fallback(handler)).await.unwrap() });
    (address, seen)
}

fn turns() -> Vec<Turn> {
    vec![("user".into(), "Hi".into()), ("assistant".into(), "Hello".into()), ("user".into(), "Best deal?".into())]
}

#[tokio::test]
async fn claude_request_and_reply() {
    let reply = json!({ "content": [{ "type": "thinking", "thinking": "x" }, { "type": "text", "text": "The paper." }] });
    let (address, seen) = stand_in(200, reply).await;
    let cfg = AiConfig { kind: Kind::Anthropic, base_url: address, api_key: "sk-test".into(), model: "claude-opus-5-5".into() };
    let answer = complete(&reqwest::Client::new(), &cfg, "You help.", &turns()).await.unwrap();
    assert_eq!(answer, "The paper.");

    let seen = seen.lock().unwrap();
    let (path, headers, body) = &seen[0];
    assert_eq!(path, "/v1/messages");
    assert_eq!(headers["x-api-key"], "sk-test");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["system"], "You help.");
    assert!(body["max_tokens"].as_u64().unwrap() > 0);
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
    assert_eq!(body["messages"][0], json!({ "role": "user", "content": "Hi" }));
}

#[tokio::test]
async fn openai_style_request_and_reply() {
    let reply = json!({ "choices": [{ "message": { "role": "assistant", "content": "<think>hm</think>The wrench." } }] });
    let (address, seen) = stand_in(200, reply).await;
    let cfg = AiConfig { kind: Kind::OpenAi, base_url: format!("{address}/v1"), api_key: "key".into(), model: "m".into() };
    let answer = complete(&reqwest::Client::new(), &cfg, "You help.", &turns()).await.unwrap();
    assert_eq!(answer, "The wrench.");

    let seen = seen.lock().unwrap();
    let (path, headers, body) = &seen[0];
    assert_eq!(path, "/v1/chat/completions");
    assert_eq!(headers["authorization"], "Bearer key");
    assert_eq!(body["messages"][0], json!({ "role": "system", "content": "You help." }));
    assert_eq!(body["messages"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn local_service_gets_no_authorization_header() {
    let (address, seen) = stand_in(200, json!({ "data": [{ "id": "b" }, { "id": "a" }] })).await;
    let cfg = AiConfig { kind: Kind::OpenAi, base_url: address, api_key: String::new(), model: String::new() };
    assert_eq!(list_models(&reqwest::Client::new(), &cfg).await.unwrap(), vec!["a", "b"]);
    assert!(!seen.lock().unwrap()[0].1.contains_key("authorization"));
}

#[tokio::test]
async fn service_errors_are_explained() {
    let (address, _) = stand_in(401, json!({ "error": { "message": "invalid x-api-key" } })).await;
    let cfg = AiConfig { kind: Kind::Anthropic, base_url: address, api_key: "bad".into(), model: "m".into() };
    let error = complete(&reqwest::Client::new(), &cfg, "s", &turns()).await.unwrap_err().to_string();
    assert_eq!(error, "127.0.0.1 answered 401: invalid x-api-key. Check the API key.");

    let (address, _) = stand_in(200, json!({ "choices": [] })).await;
    let cfg = AiConfig { kind: Kind::OpenAi, base_url: address, api_key: String::new(), model: "m".into() };
    let error = complete(&reqwest::Client::new(), &cfg, "s", &turns()).await.unwrap_err().to_string();
    assert_eq!(error, "The AI service answered, but with no text.");

    // Nothing listens on this port.
    let cfg = AiConfig { kind: Kind::OpenAi, base_url: "http://127.0.0.1:9".into(), api_key: String::new(), model: "m".into() };
    let error = complete(&reqwest::Client::new(), &cfg, "s", &turns()).await.unwrap_err().to_string();
    assert!(error.starts_with("Could not connect to 127.0.0.1"), "{error}");
}

/// A stand-in service that refuses requests carrying fields an older model
/// would not know, and answers the rest.
async fn picky_stand_in(reply: Value) -> (String, Seen) {
    use axum::extract::Request;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let handler = move |request: Request| {
        let recorder = recorder.clone();
        let reply = reply.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let extras = json.get("output_config").is_some() || json.get("response_format").is_some();
            recorder.lock().unwrap().push((parts.uri.to_string(), parts.headers, json));
            if extras {
                let refusal = json!({ "error": { "message": "output_config: Extra inputs are not permitted" } });
                (axum::http::StatusCode::BAD_REQUEST, axum::Json(refusal))
            } else {
                (axum::http::StatusCode::OK, axum::Json(reply))
            }
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, axum::Router::new().fallback(handler)).await.unwrap() });
    (address, seen)
}

fn shaped() -> Options {
    Options { json_schema: Some(json!({ "type": "object" })), quick: true }
}

#[tokio::test]
async fn quick_shaped_requests_say_so_and_chat_does_not() {
    let reply = json!({ "content": [{ "type": "text", "text": "{}" }], "stop_reason": "end_turn" });
    let (address, seen) = stand_in(200, reply).await;
    let cfg = AiConfig { kind: Kind::Anthropic, base_url: address, api_key: "k".into(), model: "claude-haiku-5-5".into() };
    let http = reqwest::Client::new();
    complete_with(&http, &cfg, "s", &turns(), &shaped()).await.unwrap();
    complete(&http, &cfg, "s", &turns()).await.unwrap();
    let seen: Vec<Value> = seen.lock().unwrap().iter().map(|(_, _, body)| body.clone()).collect();
    assert_eq!(
        seen[0]["output_config"],
        json!({ "effort": "low", "format": { "type": "json_schema", "schema": { "type": "object" } } })
    );
    assert!(seen[1].get("output_config").is_none(), "an ordinary chat turn is left to the model's defaults");
    // Thinking counts towards the limit, so both leave room for it.
    assert!(seen[0]["max_tokens"].as_u64().unwrap() >= 2048 && seen[1]["max_tokens"].as_u64().unwrap() >= 8000);

    let reply = json!({ "choices": [{ "message": { "content": "{}" } }] });
    let (address, seen) = stand_in(200, reply).await;
    let cfg = AiConfig { kind: Kind::OpenAi, base_url: address, api_key: String::new(), model: "m".into() };
    complete_with(&http, &cfg, "s", &turns(), &shaped()).await.unwrap();
    assert_eq!(seen.lock().unwrap()[0].2["response_format"], json!({ "type": "json_object" }));
}

#[tokio::test]
async fn a_service_that_rejects_the_extra_fields_is_asked_again_without_them() {
    let http = reqwest::Client::new();
    let (address, seen) = picky_stand_in(json!({ "content": [{ "type": "text", "text": "{\"match\": 0}" }] })).await;
    let cfg = AiConfig { kind: Kind::Anthropic, base_url: address, api_key: "k".into(), model: "an-older-model".into() };
    assert_eq!(complete_with(&http, &cfg, "s", &turns(), &shaped()).await.unwrap(), "{\"match\": 0}");
    assert_eq!(seen.lock().unwrap().len(), 2);

    let (address, seen) = picky_stand_in(json!({ "choices": [{ "message": { "content": "ok" } }] })).await;
    let cfg = AiConfig { kind: Kind::OpenAi, base_url: address, api_key: String::new(), model: "m".into() };
    assert_eq!(complete_with(&http, &cfg, "s", &turns(), &shaped()).await.unwrap(), "ok");
    assert_eq!(seen.lock().unwrap().len(), 2);

    // Other refusals are not retried.
    let (address, seen) = stand_in(401, json!({ "error": { "message": "invalid x-api-key" } })).await;
    let cfg = AiConfig { kind: Kind::Anthropic, base_url: address, api_key: "bad".into(), model: "m".into() };
    assert!(complete_with(&http, &cfg, "s", &turns(), &shaped()).await.is_err());
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn refusals_and_cut_off_answers_are_explained() {
    let http = reqwest::Client::new();
    let (address, _) = stand_in(200, json!({ "content": [], "stop_reason": "refusal" })).await;
    let cfg = AiConfig { kind: Kind::Anthropic, base_url: address, api_key: "k".into(), model: "m".into() };
    assert_eq!(complete(&http, &cfg, "s", &turns()).await.unwrap_err().to_string(), "The AI service declined to answer this.");

    let cut_off = json!({ "content": [{ "type": "thinking", "thinking": "" }], "stop_reason": "max_tokens" });
    let (address, _) = stand_in(200, cut_off).await;
    let cfg = AiConfig { kind: Kind::Anthropic, base_url: address, api_key: "k".into(), model: "m".into() };
    let error = complete(&http, &cfg, "s", &turns()).await.unwrap_err().to_string();
    assert_eq!(error, "The AI ran out of room before it finished answering.");
}

#[tokio::test]
async fn the_matching_model_is_used_for_matching_only() {
    let mut s = Settings {
        ai_provider: "claude".into(),
        ai_api_key: "k".into(),
        ai_model: "claude-opus-5-5".into(),
        ..Default::default()
    };
    assert_eq!(AiConfig::for_matching(&s).unwrap().model, "claude-opus-5-5", "falls back to the assistant's model");
    s.ai_match_model = " claude-haiku-5-5 ".into();
    assert_eq!(AiConfig::for_matching(&s).unwrap().model, "claude-haiku-5-5");
    assert_eq!(AiConfig::from_settings(&s, true).unwrap().model, "claude-opus-5-5");
    assert!(AiConfig::for_matching(&Settings::default()).is_err(), "no service connected");
}
