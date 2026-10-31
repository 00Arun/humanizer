//! Minimal raw-HTTP client for the Claude Messages API (no Rust SDK exists).

use serde_json::{json, Value};

pub struct ClaudeClient {
    pub http: reqwest::Client,
    pub api_key: String,
    pub model: String,
    pub effort: String,
}

impl ClaudeClient {
    /// One user turn in, assistant text out. Opts into server-side refusal
    /// fallbacks so a classifier decline is retried on a sibling model.
    pub async fn complete(&self, system: &str, user: &str, max_tokens: u32, effort_override: Option<&str>) -> Result<String, String> {
        self.chat(system, &[("user".to_string(), user.to_string())], max_tokens, effort_override).await
    }

    /// Multi-turn: `turns` alternate user/assistant and end with the user.
    pub async fn chat(&self, system: &str, turns: &[(String, String)], max_tokens: u32, effort_override: Option<&str>) -> Result<String, String> {
        let messages: Vec<Value> = turns.iter().map(|(role, content)| json!({"role": role, "content": content})).collect();
        let body = json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": messages,
            "fallbacks": "default",
            "output_config": {"effort": effort_override.unwrap_or(&self.effort)}
        });

        let resp = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("content-type", "application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2026-10-21")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Network error: {e}"))?;

        let status = resp.status();
        let payload: Value = resp.json().await.map_err(|e| format!("Bad response: {e}"))?;

        if !status.is_success() {
            let msg = payload["error"]["message"].as_str().unwrap_or("unknown error");
            return Err(match status.as_u16() {
                401 => "Invalid API key. Check Settings.".to_string(),
                429 => "Rate limited by the API. Wait a moment and try again.".to_string(),
                529 => "The API is overloaded right now. Try again shortly.".to_string(),
                code => format!("API error {code}: {msg}"),
            });
        }

        let stop = payload["stop_reason"].as_str().unwrap_or("");
        if stop == "refusal" {
            return Err("The model declined to process this text.".to_string());
        }
        let text: String = payload["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if text.trim().is_empty() {
            return Err("The model returned an empty response.".to_string());
        }
        if stop == "max_tokens" {
            return Err("The response was cut off (max_tokens). Try a shorter passage.".to_string());
        }
        Ok(text)
    }

    pub async fn ping(&self) -> Result<String, String> {
        let text = self.complete("Reply with exactly: ok", "ping", 64, Some("low")).await?;
        Ok(format!("Connected to {} ({})", self.model, text.trim()))
    }
}
