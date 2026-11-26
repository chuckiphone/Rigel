//! OpenRouter chat completions, streamed (§7.5).
//!
//! Two rules shape the implementation:
//!
//! * **Never retry after the first token.** A retried stream that already emitted is a
//!   duplicated answer in the transcript, which is worse than an error.
//! * **`Retry-After` wins over jitter.** A provider that says when to come back knows more
//!   than an exponential curve does.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::limiter::RateLimiter;
use super::local;
use super::model::{
    ChatMessage, ChatRequest, Delta, DeltaSink, Model, ModelError, RawUsage, SseDecoder,
};
use super::retry::{self, RetryClass};

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_MODEL: &str = "openai/gpt-4.1-mini";

pub struct OpenRouterModel {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    max_attempts: u32,
    /// What `id()` reports. Not sent: the wire contract is the same for every caller.
    provider: String,
    /// OpenRouter reads `usage: { "include": true }` to return cost. Gemini's compatibility
    /// layer does not document it, so its profile leaves the field out.
    request_usage: bool,
    /// The process-wide bucket for this door: every instance shares it.
    limiter: Arc<RateLimiter>,
    /// A custom door's configured header pairs (roadmap 28). Empty for every built-in row.
    headers: Vec<(String, String)>,
    /// A provider's chat-template switches, sent as `chat_template_kwargs`. llama.cpp reads them
    /// (`enable_thinking`); no hosted row defines the field, so it is `None` there.
    template_kwargs: Option<Value>,
    /// Provider credentials a router accepts for closed models. These are sent only when the
    /// requested model belongs to `conditional_prefix`; a direct call never carries them.
    conditional_headers: Vec<(String, String)>,
    conditional_prefix: Option<String>,
    /// OpenRouter routing preferences for this project (roadmap 26). `None` sends no policy.
    routing: Option<RoutingPolicy>,
}

/// Per-project OpenRouter routing preferences (roadmap 26).
///
/// The file is `<project>/.rigel/routing.json`; a missing or unreadable file is the documented
/// default (no order, fallbacks on), so a broken preference can never stop a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Provider slugs OpenRouter should try first, in order.
    #[serde(default)]
    pub order: Vec<String>,
    /// Whether OpenRouter may fall back to another provider.
    #[serde(default = "default_allow_fallbacks")]
    pub allow_fallbacks: bool,
    /// Quantisations to prefer, in order.
    #[serde(default)]
    pub quantizations: Vec<String>,
}

fn default_allow_fallbacks() -> bool {
    true
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            allow_fallbacks: true,
            quantizations: Vec::new(),
        }
    }
}

impl RoutingPolicy {
    pub fn load(project: &std::path::Path) -> Self {
        std::fs::read_to_string(project.join(".rigel/routing.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Whether this policy would add nothing to the request body.
    pub fn is_default(&self) -> bool {
        self.order.is_empty() && self.quantizations.is_empty() && self.allow_fallbacks
    }

    /// One line for the route inspector, read before the turn.
    pub fn describe(&self) -> String {
        if self.is_default() {
            return "default (no order, fallbacks on)".to_string();
        }
        let mut parts = Vec::new();
        if !self.order.is_empty() {
            parts.push(format!("order {}", self.order.join(" -> ")));
        }
        parts.push(format!(
            "fallbacks {}",
            if self.allow_fallbacks { "on" } else { "off" }
        ));
        if !self.quantizations.is_empty() {
            parts.push(format!("quantizations {}", self.quantizations.join(", ")));
        }
        parts.join(" · ")
    }

    /// The `provider` object OpenRouter reads. Empty lists are omitted rather than sent as [].
    fn wire(&self) -> Value {
        let mut field = json!({ "allow_fallbacks": self.allow_fallbacks });
        if !self.order.is_empty() {
            field["order"] = json!(self.order);
        }
        if !self.quantizations.is_empty() {
            field["quantizations"] = json!(self.quantizations);
        }
        field
    }
}

impl OpenRouterModel {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = crate::net::client::shared_provider_client("openrouter")
            .expect("default HTTP client configuration is valid")
            .client()
            .clone();
        Self {
            client,
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_attempts: 3,
            provider: "openrouter".to_string(),
            request_usage: true,
            limiter: RateLimiter::shared(),
            headers: Vec::new(),
            template_kwargs: None,
            conditional_headers: Vec::new(),
            conditional_prefix: None,
            routing: None,
        }
    }

    /// The provider name this instance reports. The wire contract is unchanged.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }

    /// Apply a provider's advertised request budget to the shared limiter.
    pub fn with_provider_rate(self, requests_per_minute: u32) -> Self {
        self.limiter
            .set_provider_rate(&self.provider, requests_per_minute);
        self
    }

    /// Drop OpenRouter's cost-request field for a provider that does not define it.
    pub fn without_usage_request(mut self) -> Self {
        self.request_usage = false;
        self
    }

    /// For tests and self-hosted gateways: same wire contract, different origin.
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let mut model = Self::new(api_key);
        model.base_url = base_url.into().trim_end_matches('/').to_string();
        model
    }

    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }

    /// A different total budget for one door. A local server processes its prompt on this CPU,
    /// where a long turn is minutes rather than seconds.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(20))
            .build()
        {
            self.client = client;
        }
        self
    }

    /// A provider's chat-template switches, sent as `chat_template_kwargs`.
    pub fn with_template_kwargs(mut self, kwargs: Value) -> Self {
        self.template_kwargs = Some(kwargs);
        self
    }

    /// A custom door's header pairs (roadmap 28).
    pub fn with_headers(mut self, headers: impl IntoIterator<Item = (String, String)>) -> Self {
        self.headers = headers.into_iter().collect();
        self
    }

    /// Headers used only when the requested model belongs to a router family. FireRouter accepts
    /// Anthropic/OpenAI keys this way; attaching them to a direct Fireworks call would disclose
    /// credentials the call does not need.
    pub fn with_conditional_headers(
        mut self,
        prefix: impl Into<String>,
        headers: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.conditional_prefix = Some(prefix.into());
        self.conditional_headers = headers.into_iter().collect();
        self
    }

    fn conditional_headers_for(&self, model: &str) -> &[(String, String)] {
        match self.conditional_prefix.as_deref() {
            Some(prefix) if router_family(model, prefix) => &self.conditional_headers,
            _ => &[],
        }
    }

    /// The per-project OpenRouter preferences (roadmap 26).
    pub fn with_policy(mut self, policy: RoutingPolicy) -> Self {
        self.routing = Some(policy);
        self
    }

    /// One chat request. A local door has no key, and an empty `Authorization: Bearer ` is not
    /// a key: the header is omitted rather than sent blank.
    pub(crate) fn request(&self, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .header("X-Title", "Rigel");
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for (name, value) in self.conditional_headers_for(model) {
            request = request.header(name, value);
        }
        let request = request.json(body);
        if self.api_key.trim().is_empty() {
            request
        } else {
            request.bearer_auth(&self.api_key)
        }
    }

    fn body(&self, request: &ChatRequest) -> Value {
        let mut body = json!({
            // Never a batch-only id: the provider would route it to its batch adapter and answer
            // 404 instead of streaming a turn.
            "model": super::model::chat_model_id(&request.model),
            "messages": request.messages,
            "stream": true,
            "temperature": request.temperature,
            "stream_options": { "include_usage": true },
        });
        // A text-only request keeps the exact serde form above and below. Only a message that
        // actually carries an image is re-encoded as an OpenAI content array.
        if request.has_images() {
            body["messages"] =
                Value::Array(request.messages.iter().map(ChatMessage::openai).collect());
        }
        if self.request_usage {
            body["usage"] = json!({ "include": true });
        }
        // OpenRouter's per-project provider preferences. A default policy adds no field, so a
        // project that never configured one sends exactly the request it sent before.
        if let Some(policy) = &self.routing
            && !policy.is_default()
        {
            body["provider"] = policy.wire();
        }
        if let Some(kwargs) = &self.template_kwargs {
            body["chat_template_kwargs"] = kwargs.clone();
        }
        if !request.tools.is_empty() {
            body["tools"] = Value::Array(request.tools.iter().map(|spec| spec.openai()).collect());
            body["tool_choice"] = json!("auto");
        }
        if let Some(tokens) = request.max_tokens {
            body["max_tokens"] = json!(tokens);
        }
        if let Some(effort) = &request.effort {
            body["reasoning_effort"] = json!(effort);
        }
        body
    }

    async fn stream_response(
        &self,
        response: reqwest::Response,
        cancel: &CancellationToken,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<(), ModelError> {
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        let mut emitted = false;
        // A reply that spent its whole budget on the reasoning scratchpad has no answer in it.
        // Tracked apart from `emitted`, which reasoning alone satisfies.
        let mut answered = false;
        let mut finish: Option<String> = None;
        let mut served_model: Option<String> = None;

        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(ModelError::Cancelled),
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else {
                break;
            };
            let bytes = chunk.map_err(|error| ModelError::Http(error.to_string()))?;
            for payload in decoder.feed(&bytes) {
                if payload.trim() == "[DONE]" {
                    return conclude(finish, answered, on_delta);
                }
                let value: Value = serde_json::from_str(&payload)
                    .map_err(|error| ModelError::Parse(format!("{error}: {payload}")))?;

                // Routers identify the concrete model on normal chunks, not necessarily on the
                // final usage chunk. Remember the first non-empty id and attach it to usage.
                if served_model.is_none() {
                    served_model = value
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|model| !model.is_empty())
                        .map(str::to_string);
                }

                if let Some(error) = value.get("error") {
                    return Err(ModelError::Other(
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("provider error")
                            .to_string(),
                    ));
                }

                if let Some(mut usage) = value.get("usage").and_then(parse_usage) {
                    if usage.served_model.is_none() {
                        usage.served_model = served_model.clone();
                    }
                    // A local door bills nothing. Its roster publishes no price, so the fallback
                    // table would otherwise invent one for a model running on this machine.
                    if local::is_local(&self.provider) {
                        usage.cost_usd = Some(0.0);
                    }
                    emitted = true;
                    on_delta(Delta::Usage(usage));
                }

                let Some(choice) = value
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|choices| choices.first())
                else {
                    continue;
                };
                let Some(delta) = choice.get("delta") else {
                    continue;
                };

                if let Some(text) = delta.get("content").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    emitted = true;
                    answered = true;
                    on_delta(Delta::Text(text.to_string()));
                }
                let reasoning = delta
                    .get("reasoning")
                    .or_else(|| delta.get("reasoning_content"))
                    .and_then(Value::as_str);
                if let Some(text) = reasoning
                    && !text.is_empty()
                {
                    emitted = true;
                    on_delta(Delta::Reasoning(text.to_string()));
                }

                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for (fallback_index, call) in calls.iter().enumerate() {
                        let index = call
                            .get("index")
                            .and_then(Value::as_u64)
                            .unwrap_or(fallback_index as u64)
                            as u32;
                        let id = call.get("id").and_then(Value::as_str);
                        let function = call.get("function");
                        let name = function.and_then(|f| f.get("name")).and_then(Value::as_str);
                        let arguments = function
                            .and_then(|f| f.get("arguments"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        emitted = true;
                        answered = true;
                        on_delta(Delta::ToolCall {
                            index,
                            id: id.map(str::to_string),
                            name: name.map(str::to_string),
                            arguments: arguments.to_string(),
                        });
                    }
                }

                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    // Held until the stream is over: `length` with no answer is an error, and a
                    // Finish row already written would tell the coordinator the turn succeeded.
                    finish = Some(reason.to_string());
                }
            }
        }

        // A stream that ends without a token and without [DONE] is a dead connection, not an
        // empty answer.
        if !emitted {
            return Err(ModelError::Parse(
                "the stream ended before any content arrived".to_string(),
            ));
        }
        conclude(finish, answered, on_delta)
    }
}

/// The end of a stream: an error for a starved answer, otherwise the provider's finish row.
///
/// llama.cpp with a reasoning template answers `finish_reason: "length"` and no `content` when
/// the output budget ran out mid-thought. Reporting that as a successful empty turn hides the
/// one fact the caller needs: the budget, not the model, ended the answer.
fn conclude(
    finish: Option<String>,
    answered: bool,
    on_delta: &mut DeltaSink<'_>,
) -> Result<(), ModelError> {
    if !answered && finish.as_deref() == Some("length") {
        return Err(ModelError::Other(
            "the model reached its output-token limit before writing an answer; raise \
             `max_tokens`, or turn off its reasoning scratchpad"
                .to_string(),
        ));
    }
    if let Some(reason) = finish {
        on_delta(Delta::Finish(reason));
    }
    Ok(())
}

#[async_trait]
impl Model for OpenRouterModel {
    fn id(&self) -> String {
        self.provider.clone()
    }

    fn supports_images(&self) -> bool {
        true
    }

    async fn chat(
        &self,
        request: ChatRequest,
        cancel: &CancellationToken,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<(), ModelError> {
        let body = self.body(&request);
        let mut attempt = 0;

        loop {
            attempt += 1;
            let permit = self.limiter.acquire(&self.provider).await;
            if permit.waited >= Duration::from_millis(10) {
                on_delta(Delta::RateLimit {
                    provider: self.provider.clone(),
                    waited_ms: permit.waited.as_millis() as u64,
                });
            }
            let send = self.request(&body).send();

            let response = tokio::select! {
                _ = cancel.cancelled() => return Err(ModelError::Cancelled),
                response = send => response,
            };

            match response {
                Ok(response) if response.status().is_success() => {
                    let result = self.stream_response(response, cancel, on_delta).await;
                    return result;
                }
                Ok(response) => {
                    let status = response.status().as_u16();
                    let retry_after = retry_after_header(&response);
                    // A chat post is retried only before the stream starts. The success arm
                    // above returns from `stream_response`, so there is no path here after a
                    // token has been emitted.
                    if let Some(event) = retry::decision(
                        RetryClass::Chat,
                        attempt,
                        self.max_attempts,
                        Some(status),
                        false,
                        false,
                        retry_after.map(Duration::from_secs),
                        250,
                    ) {
                        let wait = Duration::from_millis(event.delay_ms);
                        if status == 429 {
                            self.limiter.penalize(&self.provider, wait);
                        }
                        on_delta(Delta::Retry(event));
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    let body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "<unreadable body>".to_string());
                    return Err(ModelError::Status {
                        status,
                        body: truncate(&body, 600),
                        retry_after,
                    });
                }
                Err(error) => {
                    let transport = error.is_timeout() || error.is_connect() || error.is_request();
                    if let Some(event) = retry::decision(
                        RetryClass::Chat,
                        attempt,
                        self.max_attempts,
                        None,
                        transport,
                        false,
                        None,
                        250,
                    ) {
                        let wait = Duration::from_millis(event.delay_ms);
                        on_delta(Delta::Retry(event));
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    return Err(ModelError::Http(error.to_string()));
                }
            }
        }
    }
}

fn retry_after_header(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn parse_usage(value: &Value) -> Option<RawUsage> {
    if value.is_null() {
        return None;
    }
    let get = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = value
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning = value
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(RawUsage {
        input_tokens: get("prompt_tokens"),
        cached_input_tokens: cached,
        // These doors cache automatically and report only reads; a write count would be an
        // invention, so it stays zero.
        cache_write_input_tokens: 0,
        output_tokens: get("completion_tokens"),
        reasoning_tokens: reasoning,
        cost_usd: value.get("cost").and_then(Value::as_f64),
        served_model: None,
    })
}

/// Whether a model id names the router family. Fireworks accepts the short router id, a
/// `firerouter/...` pin, and the full resource name used by its model roster.
fn router_family(model: &str, prefix: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    let prefix = prefix.trim().to_ascii_lowercase();
    if model.is_empty() || prefix.is_empty() {
        return false;
    }
    model == prefix
        || model.starts_with(&format!("{prefix}/"))
        || model.starts_with(&format!("accounts/fireworks/routers/{prefix}"))
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::agent::image::ImagePart;
    use crate::agent::model::{ChatMessage, ChatRequest, ToolCallBuilder};
    use crate::agent::{Cancels, Coordinator, SubagentPolicy};
    use crate::{BudgetService, NullSink, SessionService, StopAt, Tools};

    /// A two-connection server: 429 with Retry-After on the first request, SSE on the second.
    async fn fake_provider() -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();

        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = vec![0u8; 8192];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buffer[..read]).to_string());

                if log.lock().unwrap().len() == 1 {
                    let _ = socket
                        .write_all(
                            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await;
                } else {
                    let events = concat!(
                        "data: {\"model\":\"openai/gpt-4.1-mini\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
                        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
                        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
                        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.txt\\\"}\"}}]}}]}\n\n",
                        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":7,\"cost\":0.00025}}\n\n",
                        "data: [DONE]\n\n",
                    );
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        events.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(events.as_bytes()).await;
                }
                let _ = socket.shutdown().await;
            }
        });

        (format!("http://{address}"), seen)
    }

    #[tokio::test]
    async fn retries_a_429_and_streams_text_tool_calls_and_cost() {
        let (base, seen) = fake_provider().await;
        let model = OpenRouterModel::with_base_url("test-key", base);
        let request = ChatRequest {
            model: "openai/gpt-4.1-mini".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: Some(64),
            temperature: 0.0,
            effort: None,
        };

        let mut text = String::new();
        let mut builder = ToolCallBuilder::default();
        let mut usage = None;
        let mut retries = Vec::new();
        let cancel = CancellationToken::new();
        let mut sink = |delta: Delta| match delta {
            Delta::Text(chunk) => text.push_str(&chunk),
            Delta::ToolCall {
                index,
                id,
                name,
                arguments,
            } => builder.apply(index, id.as_deref(), name.as_deref(), &arguments),
            Delta::Usage(raw) => usage = Some(raw),
            Delta::Retry(event) => retries.push(event),
            _ => {}
        };

        model.chat(request, &cancel, &mut sink).await.unwrap();
        assert_eq!(text, "Hello");
        let calls = builder.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, "{\"path\":\"a.txt\"}");
        let usage = usage.expect("usage chunk");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.cost_usd, Some(0.00025));
        assert_eq!(usage.served_model.as_deref(), Some("openai/gpt-4.1-mini"));
        assert_eq!(seen.lock().unwrap().len(), 2, "the 429 was retried once");
        assert_eq!(retries.len(), 1, "the retry is visible to the coordinator");
        assert_eq!(retries[0].class, RetryClass::Chat);
        assert_eq!(retries[0].attempt, 1);
        assert!(retries[0].delay_ms <= 250, "Retry-After: 0 wins");
    }

    /// The text-only shape is not merely still valid: it is the same object it was before image
    /// parts existed. There is no empty `images` key and `content` remains a string.
    #[test]
    fn a_text_message_keeps_its_exact_wire_shape() {
        let model = OpenRouterModel::new("test-key");
        let body = model.body(&ChatRequest {
            model: "openai/gpt-4.1-mini".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        });
        assert_eq!(
            serde_json::to_string(&body["messages"]).unwrap(),
            r#"[{"content":"hi","role":"user"}]"#
        );
    }

    #[test]
    fn image_parts_become_openai_text_and_image_url_content() {
        let model = OpenRouterModel::new("test-key");
        let body = model.body(&ChatRequest {
            model: "nvidia/nemotron-3.5-super-vl-120b-a12b".to_string(),
            messages: vec![ChatMessage::user_with_images(
                "read the traffic sign in this image, what does it say and what color is the sign?",
                vec![ImagePart {
                    url: "https://example.com/traffic-sign.png".to_string(),
                }],
            )],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        });
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert!(
            content[0]["text"]
                .as_str()
                .unwrap()
                .contains("traffic sign")
        );
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "https://example.com/traffic-sign.png"
        );
        assert!(
            body["messages"][0].get("images").is_none(),
            "the internal part list must not reach the provider"
        );
    }

    #[tokio::test]
    async fn a_stub_transport_receives_the_image_in_the_request_body() {
        let (base, seen) = streamed(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"stop sign\"}}]}\n\n",
            "data: [DONE]\n\n",
        ))
        .await;
        let model = OpenRouterModel::with_base_url("test-key", base);
        let request = ChatRequest {
            model: "nvidia/nemotron-3.5-super-vl-120b-a12b".to_string(),
            messages: vec![ChatMessage::user_with_images(
                "what does the sign say?",
                vec![ImagePart {
                    url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                }],
            )],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        };
        let cancel = CancellationToken::new();
        let mut sink = |_: Delta| {};
        model.chat(request, &cancel, &mut sink).await.unwrap();

        let calls = seen.lock().unwrap();
        let raw = calls.last().expect("one request");
        let body = raw
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("headers and a JSON body");
        let body: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo="
        );
    }

    /// The same image parts that reached the provider are on the persisted turn. After a
    /// service reopen, a retry builds a new turn from those parts and the stub transport sees
    /// the same `image_url` entries again, including the inlined local attachment.
    #[tokio::test]
    async fn a_persisted_image_turn_is_resent_after_reload_and_retry() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".rigel/attachments")).unwrap();
        std::fs::write(
            project.path().join(".rigel/attachments/sign.png"),
            b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR",
        )
        .unwrap();
        let images = vec![
            ImagePart::from_reference(project.path(), "https://example.com/sign.png").unwrap(),
            ImagePart::from_reference(project.path(), ".rigel/attachments/sign.png").unwrap(),
        ];
        let local_url = images[1].url.clone();

        let service = SessionService::open(project.path()).await.unwrap();
        let session = service
            .start(Some("persisted images"), Some(25))
            .await
            .unwrap();
        let agent = service
            .register_agent(&session.id, "native", Some("worker"))
            .await
            .unwrap();
        let (first_base, first_seen) = streamed(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"sign seen\"}}]}\n\n",
            "data: [DONE]\n\n",
        ))
        .await;
        let first = coordinator_for(
            project.path(),
            &service,
            Arc::new(OpenRouterModel::with_base_url("test-key", first_base)),
        );
        let run = first
            .run_with_images(&session.id, &agent.id, "read the sign", images.clone())
            .await
            .unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!(service.turn(&run.turn.id).await.unwrap().images, images);

        let first_body = sent_body(&first_seen.lock().unwrap()[0]);
        let first_content = sent_user_content(&first_body);
        assert_eq!(first_content[1]["image_url"]["url"], images[0].url);
        assert_eq!(first_content[2]["image_url"]["url"], local_url);

        // A restarted app reads the turn back from SQLite; this is the retry source.
        let reopened = SessionService::open(project.path()).await.unwrap();
        let transcript = reopened.transcript(&session.id).await.unwrap();
        let stored = &transcript
            .iter()
            .find(|entry| entry.turn.id == run.turn.id)
            .expect("the image turn is in the reloaded transcript")
            .turn;
        assert_eq!(stored.images, images);
        let retry = reopened
            .append_turn_with_images(&agent.id, stored.prompt.as_deref(), &stored.images)
            .await
            .unwrap();

        let (retry_base, retry_seen) = streamed(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"same sign\"}}]}\n\n",
            "data: [DONE]\n\n",
        ))
        .await;
        let retry_coordinator = coordinator_for(
            project.path(),
            &reopened,
            Arc::new(OpenRouterModel::with_base_url("test-key", retry_base)),
        );
        retry_coordinator
            .run_existing_with_images(
                &session.id,
                &agent.id,
                retry,
                stored.prompt.as_deref().unwrap_or("read the sign"),
                stored.images.clone(),
            )
            .await
            .unwrap();

        let retry_body = sent_body(&retry_seen.lock().unwrap()[0]);
        let retry_content = sent_user_content(&retry_body);
        assert_eq!(retry_content[1]["image_url"]["url"], images[0].url);
        assert_eq!(retry_content[2]["image_url"]["url"], local_url);
    }

    /// The coordinator the real CLI, HTTP and desktop surfaces build, reduced to the fields
    /// this test needs. No tool is called, so the tool table stays empty.
    fn coordinator_for(
        project: &std::path::Path,
        service: &SessionService,
        model: Arc<OpenRouterModel>,
    ) -> Coordinator {
        let cancels = Arc::new(Cancels::new());
        let budget = Arc::new(BudgetService::new(
            service.pool().clone(),
            cancels.clone() as Arc<dyn crate::InterruptSink>,
        ));
        let tools = Tools::new(
            project.to_path_buf(),
            service.clone(),
            budget.clone(),
            project.join("index"),
            StopAt::Never,
        );
        Coordinator {
            session: service.clone(),
            budget,
            tools,
            model,
            sink: Arc::new(NullSink),
            cancels,
            project_root: project.to_path_buf(),
            model_name: "openrouter".to_string(),
            effort: None,
            max_steps: 2,
            subagents: SubagentPolicy::default(),
            depth: 0,
            parent_cancel: None,
            worktree_dir: None,
            chat_parent: None,
        }
    }

    fn sent_body(raw: &str) -> serde_json::Value {
        let body = raw
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .expect("headers and a JSON body");
        serde_json::from_str(body).unwrap()
    }

    fn sent_user_content(body: &serde_json::Value) -> &serde_json::Value {
        let message = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .expect("a user message");
        &message["content"]
    }

    #[tokio::test]
    async fn a_cancelled_request_returns_cancelled_without_retrying() {
        let (base, seen) = fake_provider().await;
        let model = OpenRouterModel::with_base_url("test-key", base).with_attempts(1);
        let request = ChatRequest {
            model: "openai/gpt-4.1-mini".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut sink = |_: Delta| {};
        let error = model.chat(request, &cancel, &mut sink).await.unwrap_err();
        assert!(matches!(error, ModelError::Cancelled));
        assert!(seen.lock().unwrap().is_empty());
    }

    /// D28: effort is a separate field on the wire, never a suffix on the model id.
    #[tokio::test]
    async fn effort_is_sent_as_reasoning_effort() {
        let (base, seen) = fake_provider().await;
        let model = OpenRouterModel::with_base_url("test-key", base);
        let request = ChatRequest {
            model: "openai/gpt-4.1-mini".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: Some("high".to_string()),
        };
        let cancel = CancellationToken::new();
        let mut sink = |_: Delta| {};
        model.chat(request, &cancel, &mut sink).await.unwrap();

        let calls = seen.lock().unwrap();
        let body = calls.last().expect("a request body");
        assert!(body.contains("\"reasoning_effort\":\"high\""), "{body}");
        assert!(body.contains("\"model\":\"openai/gpt-4.1-mini\""), "{body}");
    }

    #[tokio::test]
    async fn a_project_policy_travels_in_the_provider_field() {
        let model = OpenRouterModel::with_base_url("test-key", "https://openrouter.ai/api/v1")
            .with_policy(RoutingPolicy {
                order: vec!["anthropic".to_string(), "openai".to_string()],
                allow_fallbacks: false,
                quantizations: vec!["fp16".to_string()],
            });
        let body = model.body(&ChatRequest {
            model: "anthropic/claude-sonnet-4.5".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        });
        assert_eq!(body["provider"]["order"][0], "anthropic");
        assert_eq!(body["provider"]["order"][1], "openai");
        assert_eq!(body["provider"]["allow_fallbacks"], false);
        assert_eq!(body["provider"]["quantizations"][0], "fp16");
    }

    #[test]
    fn provider_key_headers_are_scoped_to_the_router_family() {
        let model =
            OpenRouterModel::with_base_url("test-key", "https://api.fireworks.ai/inference/v1")
                .with_conditional_headers(
                    "firerouter",
                    [
                        ("x-anthropic-api-key".to_string(), "sk-ant-test".to_string()),
                        ("x-openai-api-key".to_string(), "sk-openai-test".to_string()),
                    ],
                );

        for router in [
            "firerouter",
            "firerouter/opus",
            "accounts/fireworks/routers/firerouter",
        ] {
            assert_eq!(
                model.conditional_headers_for(router).len(),
                2,
                "{router} is a router call"
            );
        }
        for direct in ["glm-5p3-flash", "accounts/fireworks/models/glm-5p3-flash"] {
            assert!(
                model.conditional_headers_for(direct).is_empty(),
                "{direct} must not receive Anthropic/OpenAI credentials"
            );
        }

        let chat = |id: &str| ChatRequest {
            model: id.to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        };
        let router = model
            .request(&model.body(&chat("firerouter")))
            .build()
            .expect("router request");
        assert_eq!(
            router
                .headers()
                .get("x-anthropic-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("sk-ant-test")
        );
        assert_eq!(
            router
                .headers()
                .get("x-openai-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("sk-openai-test")
        );

        let direct = model
            .request(&model.body(&chat("glm-5p3-flash")))
            .build()
            .expect("direct request");
        assert!(
            direct.headers().get("x-anthropic-api-key").is_none(),
            "a direct model must not receive a provider key"
        );
        assert!(direct.headers().get("x-openai-api-key").is_none());
    }

    #[test]
    fn a_default_policy_adds_no_provider_field_and_the_file_is_per_project() {
        let model = OpenRouterModel::new("test-key");
        let body = model.body(&ChatRequest {
            model: "openai/gpt-4.1-mini".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        });
        assert!(body.get("provider").is_none());

        let root = std::env::temp_dir().join(format!("rigel-policy-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".rigel")).unwrap();
        std::fs::write(
            root.join(".rigel/routing.json"),
            r#"{"order":["groq"],"allow_fallbacks":false,"quantizations":["int8"]}"#,
        )
        .unwrap();
        let policy = RoutingPolicy::load(&root);
        assert_eq!(policy.order, vec!["groq"]);
        assert!(!policy.allow_fallbacks);
        assert_eq!(policy.quantizations, vec!["int8"]);
        assert!(policy.describe().contains("order groq"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// The refiner's 404: an id a gateway published for its batch endpoint reached
    /// `/chat/completions`. The turn goes to the interactive twin instead.
    #[tokio::test]
    async fn a_batch_id_is_sent_without_its_suffix() {
        let (base, seen) = fake_provider().await;
        let model = OpenRouterModel::with_base_url("test-key", base);
        let request = ChatRequest {
            model: "google/gemini-4-argon:batch".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        };
        let cancel = CancellationToken::new();
        let mut sink = |_: Delta| {};
        model.chat(request, &cancel, &mut sink).await.unwrap();

        let calls = seen.lock().unwrap();
        let body = calls.last().expect("a request body");
        assert!(
            body.contains("\"model\":\"google/gemini-4-argon\""),
            "{body}"
        );
        assert!(!body.contains(":batch"), "{body}");
    }

    /// One request, one streamed body, with the request captured.
    async fn streamed(events: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let raw = read_http_request(&mut socket).await;
            log.lock().unwrap().push(raw);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                events.len()
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(events.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
        (format!("http://{address}"), seen)
    }

    /// A coordinator request includes the full system prompt and tool table, so the old
    /// single-16-KiB read cut a JSON body before its final image. Read to the declared
    /// Content-Length instead; every stub in this module can use the complete request.
    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut raw = Vec::new();
        loop {
            let mut chunk = [0u8; 8192];
            let read = socket.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..read]);
            let Some(header_end) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&raw[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if raw.len() >= header_end + 4 + content_length {
                break;
            }
        }
        String::from_utf8_lossy(&raw).to_string()
    }

    #[test]
    fn a_chat_template_switch_travels_in_the_body() {
        let model =
            OpenRouterModel::new("").with_template_kwargs(json!({ "enable_thinking": false }));
        let body = model.body(&ChatRequest {
            model: "qwen".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        });
        assert_eq!(
            body.get("chat_template_kwargs"),
            Some(&json!({ "enable_thinking": false }))
        );
        // A hosted door sends exactly the request it sent before.
        let plain = OpenRouterModel::new("test-key").body(&ChatRequest {
            model: "openai/gpt-4.1-mini".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        });
        assert!(plain.get("chat_template_kwargs").is_none());
    }

    #[tokio::test]
    async fn reasoning_that_ran_out_of_budget_is_an_error_not_an_empty_answer() {
        let (base, _) = streamed(concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking…\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        ))
        .await;
        let model = OpenRouterModel::with_base_url("", base);
        let request = ChatRequest {
            model: "qwen".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: Some(24),
            temperature: 0.0,
            effort: None,
        };
        let cancel = CancellationToken::new();
        let mut finish = None;
        let mut sink = |delta: Delta| {
            if let Delta::Finish(reason) = delta {
                finish = Some(reason);
            }
        };
        let error = model
            .chat(request, &cancel, &mut sink)
            .await
            .expect_err("a starved answer is a failure");
        assert!(error.to_string().contains("max_tokens"), "{error}");
        assert_eq!(
            finish, None,
            "a failed turn must not also report a successful finish"
        );
    }

    #[tokio::test]
    async fn a_local_turn_costs_nothing_even_though_it_publishes_no_price() {
        let (base, _) = streamed(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"local answer\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":40}}\n\n",
            "data: [DONE]\n\n",
        ))
        .await;
        let model = OpenRouterModel::with_base_url("", base).with_provider("llamacpp");
        let request = ChatRequest {
            model: "qwen".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: None,
            temperature: 0.0,
            effort: None,
        };
        let cancel = CancellationToken::new();
        let mut raw = None;
        let mut sink = |delta: Delta| {
            if let Delta::Usage(usage) = delta {
                raw = Some(usage);
            }
        };
        model.chat(request, &cancel, &mut sink).await.unwrap();
        let raw = raw.expect("a usage chunk");
        assert_eq!(raw.cost_usd, Some(0.0), "a model on this machine is free");
        let usage = super::super::model::usage_from_raw(raw, "qwen");
        assert_eq!(usage.cost_micros, 0);
    }

    #[tokio::test]
    async fn a_length_finish_after_an_answer_is_still_a_success() {
        let (base, _) = streamed(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"a short answer\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        ))
        .await;
        let model = OpenRouterModel::with_base_url("", base);
        let request = ChatRequest {
            model: "qwen".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![],
            max_tokens: Some(8),
            temperature: 0.0,
            effort: None,
        };
        let cancel = CancellationToken::new();
        let mut text = String::new();
        let mut finish = None;
        let mut sink = |delta: Delta| match delta {
            Delta::Text(chunk) => text.push_str(&chunk),
            Delta::Finish(reason) => finish = Some(reason),
            _ => {}
        };
        model.chat(request, &cancel, &mut sink).await.unwrap();
        assert_eq!(text, "a short answer");
        assert_eq!(finish.as_deref(), Some("length"));
    }
}
