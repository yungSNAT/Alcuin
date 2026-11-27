//! Provider route resolution and bounded HTTP. Dispatch never retries a POST.
use crate::{
    config::Environment,
    policy::{assert_model_allowed, nonempty, require_api_pro},
};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fmt, time::Duration};
use tokio_util::sync::CancellationToken;

pub const AZURE_DEPLOYMENT_REQUIRED: &str = "Azure mode requires --azure-deployment unless your deployment is literally gpt-5.5-pro. Pass --azure-deployment <deployment> (or set AZURE_OPENAI_DEPLOYMENT), or rerun with --provider openai/--no-azure to use api.openai.com.";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiTransport {
    Responses,
    ChatCompletions,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub model: String,
    pub provider_label: String,
    pub base_url: String,
    pub key_source: String,
    pub azure_deployment_name: Option<String>,
    pub transport: ApiTransport,
}
/// Alcuin API runs always use Pro reasoning, which the Chat Completions adapter refuses, so a run,
/// a dry run and a route plan all refuse a route through OpenRouter or a custom base URL.
pub fn require_responses(route: &Route) -> Result<()> {
    anyhow::ensure!(
        route.transport == ApiTransport::Responses,
        "--reasoning-mode requires the OpenAI or Azure OpenAI Responses API; OpenRouter and custom --base-url routes use the Chat Completions adapter."
    );
    Ok(())
}
#[derive(Clone)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}
pub fn mask_api_key(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let chars: Vec<_> = key.chars().collect();
    Some(if chars.len() <= 8 {
        format!("{}***{}", chars[0], chars[chars.len() - 1])
    } else {
        format!(
            "{}****{}",
            chars[..4].iter().collect::<String>(),
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    })
}
/// Reference runOracle missing-key error. This build reads keys from the environment only.
pub fn missing_key_message(route: &Route) -> String {
    let source = if route.azure_deployment_name.is_none() {
        route.key_source.as_str()
    } else if route.key_source.contains('|') {
        "AZURE_OPENAI_API_KEY (or OPENAI_API_KEY)"
    } else {
        "AZURE_OPENAI_API_KEY (OPENAI_API_KEY is sent only to Microsoft-run Azure OpenAI hosts)"
    };
    format!(
        "Missing {source}. Set it via the environment.{}",
        if route.model.starts_with("gpt") {
            " If you have a ChatGPT Pro subscription, retry with --engine browser (or MCP engine:\"browser\" / preset:\"chatgpt-pro-heavy\"); browser mode uses your signed-in ChatGPT session instead of an API key."
        } else {
            ""
        }
    )
}
pub fn route_target(raw: &str) -> String {
    match reqwest::Url::parse(raw) {
        Ok(url) => {
            let host = url.host_str().unwrap_or("invalid");
            let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
            let parts: Vec<_> = url
                .path_segments()
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect();
            let path = parts
                .first()
                .map(|p| format!("/{p}{}", if parts.len() > 1 { "/..." } else { "" }))
                .unwrap_or_default();
            format!("{host}{port}{path}")
        }
        // Reference formatRouteTargetForLog: the trimmed or shortened text, scheme removed.
        Err(_) => {
            let trimmed = crate::policy::trim(raw);
            let chars: Vec<char> = trimmed.chars().collect();
            let shown = if chars.len() <= 64 {
                trimmed.to_owned()
            } else {
                format!(
                    "{}\u{2026}{}",
                    chars[..32].iter().collect::<String>(),
                    chars[chars.len() - 8..].iter().collect::<String>()
                )
            };
            shown
                .strip_prefix("https://")
                .or_else(|| shown.strip_prefix("http://"))
                .unwrap_or(&shown)
                .to_owned()
        }
    }
}
/// Only HTTPS `api.openai.com` hosts use the native OpenAI Responses adapter.
fn native(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|url| {
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| host == "api.openai.com" || host.ends_with(".api.openai.com"))
    })
}
/// Key names for an Azure route. `OPENAI_API_KEY` is a fallback only for Microsoft-run Azure
/// OpenAI hosts; any other endpoint, customer-run `*.azure.com` names such as
/// `cloudapp.azure.com` and local proxies included, must be given `AZURE_OPENAI_API_KEY`.
pub(crate) fn azure_key_names(endpoint: Option<&str>) -> &'static [&'static str] {
    const MICROSOFT: [&str; 5] = [
        ".openai.azure.com",
        ".cognitiveservices.azure.com",
        ".services.ai.azure.com",
        ".openai.azure.us",
        ".cognitiveservices.azure.us",
    ];
    let microsoft = endpoint
        .and_then(|raw| reqwest::Url::parse(raw).ok())
        .is_some_and(|url| {
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| MICROSOFT.iter().any(|suffix| host.ends_with(suffix)))
        });
    if microsoft {
        &["AZURE_OPENAI_API_KEY", "OPENAI_API_KEY"]
    } else {
        &["AZURE_OPENAI_API_KEY"]
    }
}
fn is_loopback_http(url: &reqwest::Url) -> bool {
    url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}
fn openrouter(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|u| u.host_str() == Some("openrouter.ai"))
}
/// Azure options resolved per field as the Node CLI does: explicit option, then
/// `AZURE_OPENAI_*`, then user config. Runs take an explicit value as given and
/// skip only empty fallbacks; route plans and the doctor skip blank values at every
/// level. No endpoint means no Azure options.
pub fn resolve_azure(
    explicit: &Value,
    config: &Value,
    env: &Environment,
    skip_blank: bool,
) -> Option<Value> {
    let usable = |s: &&str| {
        if skip_blank {
            !crate::policy::trim(s).is_empty()
        } else {
            !s.is_empty()
        }
    };
    let field = |name: &str, var: &str| {
        let explicit = explicit[name].as_str();
        let fallbacks = [env.get(var).map(String::as_str), config[name].as_str()];
        if skip_blank {
            std::iter::once(explicit)
                .chain(fallbacks)
                .flatten()
                .find(usable)
        } else {
            explicit.or_else(|| fallbacks.into_iter().flatten().find(usable))
        }
    };
    let endpoint = field("endpoint", "AZURE_OPENAI_ENDPOINT").filter(|s| !s.is_empty())?;
    let mut azure = json!({ "endpoint": endpoint });
    for (name, var) in [
        ("deployment", "AZURE_OPENAI_DEPLOYMENT"),
        ("apiVersion", "AZURE_OPENAI_API_VERSION"),
    ] {
        if let Some(value) = field(name, var) {
            azure[name] = json!(value);
        }
    }
    Some(azure)
}
pub fn resolve_route(
    model: &str,
    options: &Value,
    env: &Environment,
) -> Result<(Route, Option<Secret>)> {
    assert_model_allowed(Some(model))?;
    let mode = options["provider"].as_str().unwrap_or("auto");
    anyhow::ensure!(
        matches!(mode, "auto" | "openai" | "azure"),
        "Provider must be auto, openai or azure."
    );
    // Azure options arrive resolved (`resolve_azure`); like the reference routing state, the
    // environment is not consulted again here.
    let azure_endpoint = nonempty(options["azure"]["endpoint"].as_str());
    if mode == "azure" && azure_endpoint.is_none() {
        bail!("--provider azure requires --azure-endpoint or AZURE_OPENAI_ENDPOINT.");
    }
    let qualified = model.contains('/');
    let azure = azure_endpoint.is_some() && mode != "openai" && (mode == "azure" || !qualified);
    let deployment = nonempty(options["azure"]["deployment"].as_str());
    if azure && deployment.is_none() {
        bail!("{AZURE_DEPLOYMENT_REQUIRED}");
    }
    let mut base = nonempty(options["baseUrl"].as_str()).map(str::to_owned);
    let router_candidate = qualified && mode != "openai" && !azure;
    if router_candidate && base.as_deref().is_some_and(|s| native(s) && !openrouter(s)) {
        base = None;
    }
    if base.is_none() && !azure {
        let configured = nonempty(env.get("OPENAI_BASE_URL").map(String::as_str));
        if !router_candidate || configured.is_some_and(|s| !native(s)) {
            base = configured.map(str::to_owned);
        }
    }
    let has_native = nonempty(env.get("OPENAI_API_KEY").map(String::as_str)).is_some();
    let has_router = nonempty(env.get("OPENROUTER_API_KEY").map(String::as_str)).is_some();
    if base.is_none()
        && !azure
        && (router_candidate || (mode != "openai" && !has_native && has_router))
    {
        base = Some("https://openrouter.ai/api/v1".into());
    }
    if base.as_deref().is_some_and(openrouter) {
        base = Some("https://openrouter.ai/api/v1".into());
    }
    let keys: &[&str] = if azure {
        azure_key_names(azure_endpoint)
    } else if mode == "openai" {
        &["OPENAI_API_KEY"]
    } else if qualified || base.as_deref().is_some_and(openrouter) {
        &["OPENROUTER_API_KEY"]
    } else {
        &["OPENAI_API_KEY"]
    };
    let key = keys
        .iter()
        .find_map(|name| nonempty(env.get(*name).map(String::as_str)).map(|key| (*name, key)));
    let base = if azure {
        format!(
            "{}/openai/v1",
            azure_endpoint
                .context("Missing Azure endpoint")?
                .trim_end_matches('/')
        )
    } else {
        base.unwrap_or_else(|| "https://api.openai.com/v1".into())
    };
    let url = reqwest::Url::parse(&base).context("Invalid provider base URL")?;
    anyhow::ensure!(
        url.scheme() == "https" || is_loopback_http(&url),
        "Provider base URL must use HTTPS (or HTTP on localhost, 127.0.0.1 or [::1])."
    );
    let transport = if !azure && !native(&base) {
        ApiTransport::ChatCompletions
    } else {
        ApiTransport::Responses
    };
    let provider_label = if azure {
        "Azure OpenAI"
    } else if openrouter(&base) {
        "OpenRouter"
    } else if transport == ApiTransport::ChatCompletions {
        "OpenAI-compatible"
    } else {
        "OpenAI"
    };
    Ok((
        Route {
            model: model.into(),
            provider_label: provider_label.into(),
            base_url: base,
            key_source: key
                .map(|(name, _)| name.to_owned())
                .unwrap_or_else(|| keys.join("|")),
            azure_deployment_name: if azure {
                deployment.map(str::to_owned)
            } else {
                None
            },
            transport,
        },
        key.map(|(_, key)| Secret(key.to_owned())),
    ))
}
pub fn build_request(
    model: &str,
    options: &Value,
    user_prompt: &str,
    system: &str,
) -> Result<Value> {
    assert_model_allowed(Some(model))?;
    require_api_pro(options["reasoningMode"].as_str())?;
    let settings = model_settings(model, options)?;
    let effort = options["reasoningEffort"]
        .as_str()
        .or(settings.reasoning_effort.as_deref());
    if let Some(effort) =
        effort.filter(|effort| !matches!(*effort, "low" | "medium" | "high" | "xhigh" | "max"))
    {
        bail!(
            "Reasoning effort \"{effort}\" is not supported for {model}. Expected low, medium, high, xhigh, or max."
        );
    }
    // Reference buildRequestBody order: effort before mode.
    let mut reasoning = json!({});
    if let Some(effort) = effort {
        reasoning["effort"] = json!(effort);
    }
    reasoning["mode"] = json!("pro");
    let mut body = json!({"model":settings.api_model,"instructions":system,"input":[{"role":"user","content":[{"type":"input_text","text":user_prompt}]}],"reasoning":reasoning});
    if options["search"].as_bool() != Some(false) {
        body["tools"] = json!([{"type":"web_search"}]);
    }
    if let Some(max) = options.get("maxOutput") {
        body["max_output_tokens"] = max.clone();
    }
    let background = options["background"].as_bool().unwrap_or(true);
    if background {
        body["background"] = json!(true);
    }
    if background || options.get("previousResponseId").is_some() {
        body["store"] = json!(true);
    }
    if let Some(previous) = options["previousResponseId"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        body["previous_response_id"] = json!(previous);
    }
    Ok(body)
}
#[derive(Debug, Clone)]
pub struct ModelSettings {
    pub api_model: String,
    pub reasoning_effort: Option<String>,
    pub input_limit: u64,
    pub pricing: Option<(f64, f64)>,
}
pub fn model_settings(model: &str, options: &Value) -> Result<ModelSettings> {
    let mut settings = ModelSettings {
        api_model: model.into(),
        reasoning_effort: Some("xhigh".into()),
        input_limit: 272000,
        // GPT-6 Astra's standard input and output rates per 1M tokens on
        // https://platform.openai.com/docs/pricing, read 15 November 2026.
        pricing: Some((10.0 / 1_000_000.0, 50.0 / 1_000_000.0)),
    };
    if model == "gpt-6-astra"
        && let Some(overrides) = options["modelOverrides"][model].as_object()
    {
        if let Some(model) = overrides
            .get("apiModel")
            .and_then(Value::as_str)
            .map(crate::policy::trim)
            .filter(|model| !model.is_empty())
        {
            assert_model_allowed(Some(model))?;
            settings.api_model = model.into();
        }
        if let Some(reasoning) = overrides.get("reasoning") {
            if reasoning.is_null() {
                settings.reasoning_effort = None;
            } else if let Some(effort) = reasoning["effort"].as_str().filter(|effort| {
                matches!(
                    *effort,
                    "none" | "low" | "medium" | "high" | "xhigh" | "max"
                )
            }) {
                settings.reasoning_effort = Some(effort.into());
            }
        }
        if let Some(limit) = overrides
            .get("inputLimit")
            .and_then(Value::as_f64)
            .filter(|limit| {
                limit.is_finite()
                    && *limit > 0.0
                    && *limit <= 9_007_199_254_740_991.0
                    && limit.fract() == 0.0
            })
        {
            settings.input_limit = limit as u64;
        }
        if let Some(pricing) = overrides.get("pricing") {
            if pricing.is_null() {
                settings.pricing = None;
            } else if let (Some(input), Some(output)) = (
                pricing["inputPerToken"].as_f64(),
                pricing["outputPerToken"].as_f64(),
            ) && input.is_finite()
                && input >= 0.0
                && output.is_finite()
                && output >= 0.0
            {
                settings.pricing = Some((input, output));
            }
        }
    }
    Ok(settings)
}
/// Cached input and cache writes as multiples of the input rate, for GPT-5.6 and later models other
/// than GPT-6.1 Sol and Terra (https://platform.openai.com/docs/guides/prompt-caching, read 15
/// November 2026). Requests set no cache retention, so writes take the default 30-minute rate. For
/// GPT-6 Astra these give the $1.00 and $12.50 per 1M tokens its pricing page lists.
const CACHED_INPUT_RATE: f64 = 0.1;
pub(crate) const CACHE_WRITE_RATE: f64 = 1.25;
pub fn usage_summary(response: &Value, estimated_input: u64, pricing: Option<(f64, f64)>) -> Value {
    let input = response["usage"]["input_tokens"]
        .as_u64()
        .unwrap_or(estimated_input);
    let output = response["usage"]["output_tokens"].as_u64().unwrap_or(0);
    let reasoning = response["usage"]["reasoning_tokens"]
        .as_u64()
        .or(response["usage"]["output_tokens_details"]["reasoning_tokens"].as_u64())
        .unwrap_or(0);
    let mut usage = json!({"inputTokens":input,"outputTokens":output,"reasoningTokens":reasoning,"totalTokens":response["usage"]["total_tokens"].as_u64().unwrap_or(input.saturating_add(output))});
    let details = &response["usage"]["input_tokens_details"];
    let cached = details["cached_tokens"].as_u64().unwrap_or(0);
    let written = details["cache_write_tokens"].as_u64().unwrap_or(0);
    // Input tokens are uncached, cached or written, each at its own rate; reasoning is already in
    // output. A cache split larger than the input is inconsistent, so it gets no cost. An unpriced
    // run stores a null cost, so readers do not estimate one; only a session stored without the
    // key, as Oracle writes an unpriced one, is priced from the bundled table.
    usage["cost"] = Value::Null;
    if let Some((input_rate, output_rate)) = pricing
        && let Some(uncached) = cached
            .checked_add(written)
            .and_then(|used| input.checked_sub(used))
    {
        let input_cost =
            uncached as f64 + cached as f64 * CACHED_INPUT_RATE + written as f64 * CACHE_WRITE_RATE;
        usage["cost"] = json!(input_cost * input_rate + output as f64 * output_rate);
    }
    usage
}
/// One dispatch and read-only retrieval, shared by the HTTP client and synthetic providers.
pub trait ResponseProvider: Sync {
    fn route(&self) -> &Route;
    fn create(
        &self,
        body: &Value,
        cancel: &CancellationToken,
    ) -> impl std::future::Future<Output = Result<Value>> + Send;
    fn retrieve(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> impl std::future::Future<Output = Result<Value>> + Send;
    fn create_observed(
        &self,
        body: &Value,
        cancel: &CancellationToken,
        observer: &dyn ResponseObserver,
    ) -> impl std::future::Future<Output = Result<Value>> + Send {
        async move {
            let response = self.create(body, cancel).await?;
            observer.response(&response)?;
            Ok(response)
        }
    }
}
pub trait ResponseObserver: Send + Sync {
    fn response(&self, response: &Value) -> Result<()>;
    fn delta(&self, text: &str) -> Result<()>;
    /// Progress lines (background status, heartbeat); never part of the answer.
    fn log(&self, _line: &str) -> Result<()> {
        Ok(())
    }
}
struct IgnoreResponse;
impl ResponseObserver for IgnoreResponse {
    fn response(&self, _: &Value) -> Result<()> {
        Ok(())
    }
    fn delta(&self, _: &str) -> Result<()> {
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureReason {
    ClientTimeout,
    ConnectionLost,
    ClientAbort,
    UnsupportedEndpoint,
    ApiError,
    Unknown,
}
#[derive(Debug)]
pub struct ProviderError {
    pub reason: FailureReason,
    pub message: String,
    pub request_id: Option<String>,
    pub status: Option<u16>,
}
impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ProviderError {}
fn failure(
    reason: FailureReason,
    message: &str,
    request_id: Option<String>,
    status: Option<u16>,
) -> anyhow::Error {
    ProviderError {
        reason,
        message: message.into(),
        request_id,
        status,
    }
    .into()
}
pub fn timeout_error(message: &str) -> anyhow::Error {
    failure(FailureReason::ClientTimeout, message, None, None)
}
pub fn cancelled_error(message: &str) -> anyhow::Error {
    failure(FailureReason::ClientAbort, message, None, None)
}
/// Reference describeTransportError, with the overall deadline for client timeouts.
pub fn describe(error: &ProviderError, deadline: Option<Duration>) -> String {
    match error.reason {
        FailureReason::ClientTimeout => match deadline {
            Some(deadline) => format!(
                "Client-side timeout: OpenAI streaming call exceeded the {} deadline.",
                crate::execution::format_elapsed(deadline)
            ),
            None => "Client-side timeout: OpenAI streaming call exceeded the configured deadline."
                .into(),
        },
        FailureReason::ConnectionLost => {
            "Connection to OpenAI ended unexpectedly before the response completed.".into()
        }
        FailureReason::ClientAbort => {
            "Request was aborted before OpenAI completed the response.".into()
        }
        FailureReason::ApiError => error.message.clone(),
        FailureReason::UnsupportedEndpoint => "The Responses API returned 404/405 \u{2014} your base URL/gateway probably lacks /v1/responses (check OPENAI_BASE_URL or switch to api.openai.com / browser engine).".into(),
        FailureReason::Unknown => {
            "OpenAI streaming call ended with an unknown transport error.".into()
        }
    }
}
/// The OpenAI SDK APIError message for a body without `error.message`.
fn status_message(status: u16, bytes: &[u8]) -> String {
    let detail = match serde_json::from_slice::<Value>(bytes) {
        Ok(body) => match &body["error"] {
            Value::Null | Value::Bool(false) => None,
            Value::String(text) if text.is_empty() => None,
            error => Some(match &error["message"] {
                Value::String(text) if !text.is_empty() => text.clone(),
                Value::Null | Value::Bool(false) => error.to_string(),
                Value::String(_) => error.to_string(),
                message => message.to_string(),
            }),
        },
        Err(_) => Some(String::from_utf8_lossy(bytes).into_owned()).filter(|text| !text.is_empty()),
    };
    match detail {
        Some(detail) => format!("{status} {detail}"),
        None => format!("{status} status code (no body)"),
    }
}
/// The OpenAI SDK retry delay: `retry-after-ms`, then `retry-after` seconds or date, when
/// under a minute. A date counts from `now_ms`, milliseconds since the Unix epoch.
fn retry_after(headers: &reqwest::header::HeaderMap, now_ms: i64) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let millis = header("retry-after-ms")
        .and_then(|value| value.trim().parse::<f64>().ok())
        .or_else(|| {
            let value = header("retry-after")?.trim();
            value
                .parse::<f64>()
                .map(|seconds| seconds * 1000.0)
                .ok()
                .or_else(|| {
                    jiff::fmt::rfc2822::parse(value)
                        .ok()
                        .map(|date| (date.timestamp().as_millisecond() - now_ms) as f64)
                })
        })?;
    (millis.is_finite() && (0.0..60_000.0).contains(&millis))
        .then(|| Duration::from_millis(millis as u64))
}
pub fn retryable_retrieval(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ProviderError>().is_some_and(|error| {
        matches!(
            error.reason,
            FailureReason::ConnectionLost | FailureReason::ClientTimeout
        )
    })
}
pub fn response_metadata(response: &Value) -> Value {
    let mut metadata = json!({});
    for (target, source) in [
        ("responseId", "id"),
        ("status", "status"),
        ("requestId", "_request_id"),
    ] {
        if let Some(value) = response.get(source) {
            metadata[target] = value.clone();
        }
    }
    if let Some(reason) = response["incomplete_details"]
        .get("reason")
        .filter(|reason| !reason.is_null())
    {
        metadata["incompleteReason"] = reason.clone();
    }
    metadata
}
pub fn valid_response_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
#[derive(Debug)]
pub struct HttpProvider {
    client: reqwest::Client,
    route: Route,
    key: Secret,
}
impl HttpProvider {
    pub fn new(route: Route, key: Secret, timeout: Duration) -> Result<Self> {
        let client = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { client, route, key })
    }
    pub fn route(&self) -> &Route {
        &self.route
    }
    pub async fn create(&self, body: &Value, cancel: &CancellationToken) -> Result<Value> {
        self.create_observed(body, cancel, &IgnoreResponse).await
    }
    pub async fn create_observed(
        &self,
        body: &Value,
        cancel: &CancellationToken,
        observer: &dyn ResponseObserver,
    ) -> Result<Value> {
        let streaming = body["background"] != true;
        let (path, mut body) = match self.route.transport {
            ApiTransport::Responses => ("responses", body.clone()),
            ApiTransport::ChatCompletions => ("chat/completions", chat_request(body)?),
        };
        if streaming || self.route.transport == ApiTransport::ChatCompletions {
            body["stream"] = json!(streaming);
        }
        self.request(
            reqwest::Method::POST,
            path,
            Some(&body),
            cancel,
            streaming,
            observer,
        )
        .await
    }
    pub async fn retrieve(&self, id: &str, cancel: &CancellationToken) -> Result<Value> {
        anyhow::ensure!(
            self.route.transport == ApiTransport::Responses,
            "retrieve is not supported for OpenRouter chat/completions fallback."
        );
        anyhow::ensure!(
            !id.is_empty()
                && id.len() <= 256
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
            "Invalid response ID"
        );
        self.request(
            reqwest::Method::GET,
            &format!("responses/{id}"),
            None,
            cancel,
            false,
            &IgnoreResponse,
        )
        .await
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        cancel: &CancellationToken,
        streaming: bool,
        observer: &dyn ResponseObserver,
    ) -> Result<Value> {
        let url = format!("{}/{path}", self.route.base_url.trim_end_matches('/'));
        let abort = || {
            failure(
                FailureReason::ClientAbort,
                "The request was aborted before OpenAI finished responding.",
                None,
                None,
            )
        };
        let mut attempt = 0u32;
        let response = loop {
            let mut request = self
                .client
                .request(method.clone(), &url)
                .bearer_auth(self.key.expose());
            if let Some(body) = body {
                request = request.json(body);
            }
            let sent = tokio::select! {biased; _ = cancel.cancelled()=>return Err(abort()),result=request.send()=>result};
            // The reference SDK client retries twice; only the read-only retrieve does so here,
            // because a POST is never replayed.
            let retry = method == reqwest::Method::GET
                && attempt < 2
                && match &sent {
                    Ok(response) => match response
                        .headers()
                        .get("x-should-retry")
                        .and_then(|value| value.to_str().ok())
                    {
                        Some("true") => true,
                        Some("false") => false,
                        _ => {
                            matches!(response.status().as_u16(), 408 | 409 | 429)
                                || response.status().is_server_error()
                        }
                    },
                    Err(_) => true,
                };
            if !retry {
                break sent.map_err(|error| {
                    failure(
                        if error.is_timeout() {
                            FailureReason::ClientTimeout
                        } else {
                            FailureReason::ConnectionLost
                        },
                        if error.is_timeout() {
                            "OpenAI request timed out before completion."
                        } else {
                            "Connection to OpenAI dropped before the response completed."
                        },
                        None,
                        None,
                    )
                })?;
            }
            // shortcut: the SDK's up-to-25% jitter is omitted; the base delays match.
            let delay = sent
                .as_ref()
                .ok()
                .and_then(|response| {
                    retry_after(response.headers(), jiff::Timestamp::now().as_millisecond())
                })
                .unwrap_or_else(|| Duration::from_millis((500u64 << attempt).min(8000)));
            attempt += 1;
            tokio::select! {biased; _ = cancel.cancelled()=>return Err(abort()),_=tokio::time::sleep(delay)=>{}}
        };
        let status = response.status();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        // A successful stream is bounded per event instead (response_stream::SseDecoder).
        let buffered = !(streaming && status.is_success());
        if buffered
            && response
                .content_length()
                .is_some_and(|n| n > 8 * 1024 * 1024)
        {
            bail!("Provider response exceeds the 8 MiB transfer limit.");
        }
        let mut bytes = vec![];
        let mut size = 0usize;
        let mut decoder = crate::response_stream::SseDecoder::default();
        let mut accumulator = crate::response_stream::ResponseAccumulator::default();
        let mut chat = json!({"output_text":[""],"status":"completed","usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}});
        let mut stream_done = false;
        let mut chunks = response.bytes_stream();
        loop {
            let next = tokio::select! {biased; _ = cancel.cancelled()=>return Err(failure(FailureReason::ClientAbort,"The request was aborted before OpenAI finished responding.",request_id.clone(),None)),next=chunks.next()=>next};
            let Some(next) = next else { break };
            let next = next.map_err(|error| {
                failure(
                    if error.is_timeout() {
                        FailureReason::ClientTimeout
                    } else {
                        FailureReason::ConnectionLost
                    },
                    "Provider transfer was interrupted; request evidence is retained.",
                    request_id.clone(),
                    None,
                )
            })?;
            size += next.len();
            if buffered && size > 8 * 1024 * 1024 {
                bail!("Provider response exceeds the 8 MiB transfer limit.");
            }
            if streaming && status.is_success() {
                for event in decoder.push(&next)? {
                    self.stream_event(
                        event,
                        &mut accumulator,
                        &mut chat,
                        &mut stream_done,
                        &request_id,
                        observer,
                    )?;
                }
            } else {
                bytes.extend_from_slice(&next);
            }
        }
        if !status.is_success() {
            let error = serde_json::from_slice::<Value>(&bytes).unwrap_or_default();
            let message = if matches!(status.as_u16(), 404 | 405) {
                "HTTP 404/405 from the Responses API; this base URL or gateway likely does not expose /v1/responses. Set OPENAI_BASE_URL to api.openai.com/v1, update your Azure API version/deployment, or use the browser engine.".to_owned()
            } else {
                match error["error"]["message"].as_str() {
                    Some(message) if !message.is_empty() => message.to_owned(),
                    _ => status_message(status.as_u16(), &bytes),
                }
            };
            return Err(failure(
                if matches!(status.as_u16(), 404 | 405) {
                    FailureReason::UnsupportedEndpoint
                } else {
                    FailureReason::ApiError
                },
                &message,
                request_id,
                Some(status.as_u16()),
            ));
        }
        if streaming {
            for event in decoder.finish()? {
                self.stream_event(
                    event,
                    &mut accumulator,
                    &mut chat,
                    &mut stream_done,
                    &request_id,
                    observer,
                )?;
            }
            return match self.route.transport {
                ApiTransport::Responses => accumulator.finish(),
                ApiTransport::ChatCompletions => {
                    if chat.get("id").is_none() {
                        chat["id"] = json!(format!(
                            "openrouter-{}",
                            jiff::Timestamp::now().as_millisecond()
                        ));
                    }
                    observer.response(&chat)?;
                    Ok(chat)
                }
            };
        }
        let mut result: Value = serde_json::from_slice(&bytes).map_err(|_| {
            failure(
                FailureReason::Unknown,
                "Provider returned malformed JSON; request evidence is retained.",
                request_id.clone(),
                Some(status.as_u16()),
            )
        })?;
        if self.route.transport == ApiTransport::ChatCompletions {
            result = chat_response(&result);
        }
        if let Some(request_id) = request_id {
            result["_request_id"] = json!(request_id);
        }
        observer.response(&result)?;
        Ok(result)
    }
    fn stream_event(
        &self,
        event: crate::response_stream::Event,
        accumulator: &mut crate::response_stream::ResponseAccumulator,
        chat: &mut Value,
        done: &mut bool,
        request_id: &Option<String>,
        observer: &dyn ResponseObserver,
    ) -> Result<()> {
        if *done {
            return Ok(());
        }
        if event.data.starts_with("[DONE]") {
            *done = true;
            return Ok(());
        }
        let mut event_value: Value =
            serde_json::from_str(&event.data).context("Provider stream contains malformed JSON")?;
        if event_value.get("error").is_some()
            || event.kind == "error"
            || event_value["type"] == "error"
        {
            return Err(failure(
                FailureReason::ApiError,
                event_value["error"]["message"]
                    .as_str()
                    .or(event_value["message"].as_str())
                    .unwrap_or("Provider stream error"),
                request_id.clone(),
                None,
            ));
        }
        if self.route.transport == ApiTransport::ChatCompletions {
            if let Some(id) = event_value.get("id") {
                chat["id"] = id.clone();
            }
            let delta = event_value["choices"][0]["delta"]["content"]
                .as_str()
                .unwrap_or("");
            chat["output_text"][0] = json!(format!(
                "{}{delta}",
                chat["output_text"][0].as_str().unwrap_or("")
            ));
            if let Some(usage) = event_value.get("usage").filter(|value| !value.is_null()) {
                chat["usage"] = chat_usage(usage);
            }
            if let Some(request_id) = request_id {
                chat["_request_id"] = json!(request_id);
            }
            observer.response(chat)?;
            if !delta.is_empty() {
                observer.delta(delta)?;
            }
        } else {
            if event_value.get("type").is_none() && !event.kind.is_empty() {
                event_value["type"] = json!(event.kind);
            }
            let delta = accumulator.event(&event_value)?;
            if let Some(snapshot) = &mut accumulator.snapshot {
                if let Some(request_id) = request_id {
                    snapshot["_request_id"] = json!(request_id);
                }
                observer.response(snapshot)?;
            }
            if let Some(delta) = delta {
                observer.delta(&delta)?;
            }
        }
        Ok(())
    }
}
impl ResponseProvider for HttpProvider {
    fn route(&self) -> &Route {
        self.route()
    }
    async fn create(&self, body: &Value, cancel: &CancellationToken) -> Result<Value> {
        self.create(body, cancel).await
    }
    async fn retrieve(&self, id: &str, cancel: &CancellationToken) -> Result<Value> {
        self.retrieve(id, cancel).await
    }
    async fn create_observed(
        &self,
        body: &Value,
        cancel: &CancellationToken,
        observer: &dyn ResponseObserver,
    ) -> Result<Value> {
        self.create_observed(body, cancel, observer).await
    }
}
pub fn chat_request(body: &Value) -> Result<Value> {
    anyhow::ensure!(
        body["reasoning"].get("mode").is_none(),
        "--reasoning-mode requires the OpenAI or Azure OpenAI Responses API; OpenRouter and custom --base-url routes use the Chat Completions adapter."
    );
    let mut messages = vec![];
    if let Some(instructions) = body["instructions"]
        .as_str()
        .filter(|text| !text.is_empty())
    {
        messages.push(json!({"role":"system","content":instructions}));
    }
    for input in body["input"]
        .as_array()
        .context("Request has no input array")?
    {
        let text = input["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|content| content["type"] == "input_text")
            .filter_map(|content| content["text"].as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        messages.push(json!({"role":input["role"].as_str().unwrap_or("user"),"content":text}));
    }
    let mut request = json!({"model":body["model"],"messages":messages});
    if let Some(max) = body.get("max_output_tokens") {
        request["max_tokens"] = max.clone();
    }
    if let Some(effort) = body["reasoning"].get("effort") {
        request["reasoning_effort"] = effort.clone();
    }
    Ok(request)
}
fn chat_usage(usage: &Value) -> Value {
    let mut result = json!({"input_tokens":usage["prompt_tokens"].as_u64().unwrap_or(0),"output_tokens":usage["completion_tokens"].as_u64().unwrap_or(0),"total_tokens":usage["total_tokens"].as_u64().unwrap_or(0)});
    if let Some(reasoning) = usage["completion_tokens_details"].get("reasoning_tokens") {
        result["reasoning_tokens"] = reasoning.clone();
    }
    result
}
pub fn chat_response(response: &Value) -> Value {
    let text = response["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    json!({"id":response["id"].as_str().map(str::to_owned).unwrap_or_else(||format!("openrouter-{}",jiff::Timestamp::now().as_millisecond())),"status":"completed","output_text":[text],"output":[{"type":"text","text":text}],"usage":chat_usage(&response["usage"])})
}
pub fn text_output(response: &Value) -> String {
    if let Some(parts) = response["output_text"]
        .as_array()
        .filter(|parts| !parts.is_empty())
    {
        return parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n");
    }
    let mut parts = vec![];
    for entry in response["output"].as_array().into_iter().flatten() {
        if let Some(content) = entry["content"].as_array() {
            for part in content {
                if matches!(part["type"].as_str(), Some("output_text" | "text"))
                    && let Some(text) = part["text"].as_str().filter(|text| !text.is_empty())
                {
                    parts.push(text.to_owned());
                }
            }
        } else if let Some(text) = entry["text"].as_str() {
            parts.push(text.to_owned());
        }
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_reads_milliseconds_seconds_and_dates_under_a_minute() {
        // 2026-10-26T01:02:03Z, a Monday.
        let now_ms = 1_792_976_523_000;
        let delay = |name: &str, value: &str| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
            retry_after(&headers, now_ms)
        };
        let ms = |millis: u64| Some(Duration::from_millis(millis));
        assert_eq!(delay("retry-after-ms", "1500"), ms(1500));
        assert_eq!(delay("retry-after", " 2.5 "), ms(2500));
        assert_eq!(delay("retry-after", "60"), None);
        assert_eq!(
            delay("retry-after", "Mon, 26 Oct 2026 01:02:33 GMT"),
            ms(30_000)
        );
        assert_eq!(
            delay("retry-after", "Mon, 26 Oct 2026 01:02:33 +0000"),
            ms(30_000)
        );
        assert_eq!(
            delay("retry-after", "Mon, 26 Oct 2026 12:02:33 +1100"),
            ms(30_000)
        );
        assert_eq!(delay("retry-after", "Mon, 26 Oct 2026 01:03:03 GMT"), None);
        assert_eq!(delay("retry-after", "Mon, 26 Oct 2026 01:02:02 GMT"), None);
        assert_eq!(delay("retry-after", "Tue, 26 Oct 2026 01:02:33 GMT"), None);
        assert_eq!(delay("retry-after", "soon"), None);
    }

    /// GPT-6 Astra's standard rates per 1M tokens on https://platform.openai.com/docs/pricing,
    /// read 15 November 2026: $10.00 input, $1.00 cached input, $12.50 for a 30-minute cache write
    /// (the default retention) and $50.00 output.
    #[test]
    fn cached_and_written_input_tokens_take_their_published_rates() {
        let pricing = model_settings("gpt-6-astra", &Value::Null).unwrap().pricing;
        let cost = |details: Value| {
            usage_summary(
                &json!({"usage":{"input_tokens":1_000_000,"output_tokens":100_000,"input_tokens_details":details}}),
                0,
                pricing,
            )["cost"]
                .as_f64()
        };
        let close = |actual: Option<f64>, expected: f64| {
            assert!(
                actual.is_some_and(|actual| (actual - expected).abs() < 1e-9),
                "{actual:?} != {expected}"
            );
        };
        // 500,000 uncached at $10, 300,000 cached at $1, 200,000 written at $12.50, then output.
        close(
            cost(json!({"cached_tokens":300_000,"cache_write_tokens":200_000})),
            5.0 + 0.3 + 2.5 + 5.0,
        );
        close(cost(json!({})), 10.0 + 5.0);
        // A cache split larger than the input is inconsistent, so it is not priced, even when
        // the counts would overflow. The stored null says so, so no reader estimates a cost.
        let unpriced =
            |response: Value, pricing| usage_summary(&response, 0, pricing).get("cost").cloned();
        assert_eq!(
            unpriced(
                json!({"usage":{"input_tokens":1_000_000,"output_tokens":0,"input_tokens_details":{"cached_tokens":900_000,"cache_write_tokens":200_000}}}),
                pricing
            ),
            Some(Value::Null)
        );
        assert_eq!(
            unpriced(
                json!({"usage":{"input_tokens":u64::MAX,"output_tokens":0,"input_tokens_details":{"cached_tokens":u64::MAX,"cache_write_tokens":1}}}),
                pricing
            ),
            Some(Value::Null)
        );
        // A `pricing: null` override prices nothing either.
        assert_eq!(
            unpriced(
                json!({"usage":{"input_tokens":1_000,"output_tokens":0}}),
                None
            ),
            Some(Value::Null)
        );
        // Without usage, the estimated input is priced at the input rate.
        close(
            usage_summary(&json!({}), 1_000, pricing)["cost"].as_f64(),
            0.01,
        );
        // A pricing override keeps the cache multiples: 500,000 uncached at $2, 300,000 cached at
        // $0.20 and 200,000 written at $2.50 per 1M tokens, then 100,000 output at $8.
        let overridden = model_settings(
            "gpt-6-astra",
            &json!({"modelOverrides":{"gpt-6-astra":{"pricing":{"inputPerToken":2e-6,"outputPerToken":8e-6}}}}),
        )
        .unwrap()
        .pricing;
        close(
            usage_summary(
                &json!({"usage":{"input_tokens":1_000_000,"output_tokens":100_000,"input_tokens_details":{"cached_tokens":300_000,"cache_write_tokens":200_000}}}),
                0,
                overridden,
            )["cost"]
                .as_f64(),
            1.0 + 0.06 + 0.5 + 0.8,
        );
    }

    #[test]
    fn non_openai_and_unparseable_hosts_are_not_treated_as_native_responses() {
        for url in [
            "https://api.openai.com/v1",
            "https://eu.api.openai.com/v1",
            "https://API.OPENAI.COM/v1",
        ] {
            assert!(native(url), "{url}");
            let (route, _) =
                resolve_route("gpt-6-astra", &json!({"baseUrl": url}), &Environment::new())
                    .unwrap();
            assert_eq!(route.transport, ApiTransport::Responses, "{url}");
            require_responses(&route).unwrap();
        }
        for url in [
            "https://api.anthropic.com/v1",
            "https://generativelanguage.googleapis.com/v1",
            "https://api.x.ai/v1",
            "https://evil-api.openai.com/v1",
            "https://api.openai.com.evil.example/v1",
            "https://api.openai.com@evil.example/v1",
            "https://user:pass@evil.example/v1?host=api.openai.com",
            "https://evil.example/api.openai.com/v1",
            "https://api.openai.com./v1",
            "https://[::1]/v1",
            "https://127.0.0.1/v1",
            "http://api.openai.com/v1",
            "not a url",
            "",
        ] {
            assert!(!native(url), "{url}");
        }
        for url in [
            "https://api.anthropic.com/v1",
            "https://generativelanguage.googleapis.com/v1",
            "https://api.x.ai/v1",
            "https://evil-api.openai.com/v1",
            "https://api.openai.com@evil.example/v1",
        ] {
            let (route, _) =
                resolve_route("gpt-6-astra", &json!({"baseUrl": url}), &Environment::new())
                    .unwrap();
            assert_eq!(route.transport, ApiTransport::ChatCompletions, "{url}");
            assert!(require_responses(&route).is_err(), "{url}");
        }
        assert!(
            resolve_route(
                "gpt-6-astra",
                &json!({"baseUrl": "not a url"}),
                &Environment::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn remote_http_urls_are_rejected_and_explicit_loopback_http_is_allowed() {
        for url in [
            "http://api.openai.com/v1",
            "http://example.com/v1",
            "http://127.0.0.2:8080/v1",
            "http://localhost.example:8080/v1",
            "http://localhost.:8080/v1",
            "http://[::2]:8080/v1",
            "http://user@evil.example@localhost.example/v1",
            "ftp://api.openai.com/v1",
        ] {
            assert!(
                resolve_route("gpt-6-astra", &json!({"baseUrl": url}), &Environment::new(),)
                    .is_err(),
                "{url}"
            );
        }
        assert!(
            resolve_route(
                "openai/gpt-6-astra",
                &json!({"baseUrl": "http://api.openai.com/v1"}),
                &Environment::new(),
            )
            .is_err()
        );
        assert!(
            resolve_route(
                "gpt-6-astra",
                &json!({"azure": {"endpoint": "http://example.openai.azure.com", "deployment": "d"}}),
                &Environment::new(),
            )
            .is_err()
        );
        for url in [
            "http://127.0.0.1:8080/v1",
            "http://localhost:8080/v1",
            "http://[::1]:8080/v1",
        ] {
            let (route, _) =
                resolve_route("gpt-6-astra", &json!({"baseUrl": url}), &Environment::new())
                    .unwrap();
            assert_eq!(route.transport, ApiTransport::ChatCompletions, "{url}");
        }
        let (azure, _) = resolve_route(
            "gpt-6-astra",
            &json!({"azure": {"endpoint": "http://127.0.0.1:8080", "deployment": "d"}}),
            &Environment::new(),
        )
        .unwrap();
        assert_eq!(azure.transport, ApiTransport::Responses);
    }
    #[test]
    fn the_openai_key_reaches_only_microsoft_run_azure_hosts() {
        let openai = Environment::from([("OPENAI_API_KEY".into(), "PUBLIC-fixture".into())]);
        let route = |endpoint: &str, env: &Environment| {
            resolve_route(
                "gpt-6-astra",
                &json!({"azure": {"endpoint": endpoint, "deployment": "d"}}),
                env,
            )
            .unwrap()
        };
        for endpoint in [
            "https://PUBLIC.openai.azure.com/",
            "https://public.cognitiveservices.azure.com",
            "https://public.services.ai.azure.com",
            "https://public.openai.azure.us",
            "https://public.cognitiveservices.azure.us",
            "https://PUBLIC.OPENAI.AZURE.COM",
            "https://public.openai.azure.com:8443/path",
            "https://public.invalid@public.openai.azure.com",
        ] {
            let (route, key) = route(endpoint, &openai);
            assert_eq!(
                (route.key_source.as_str(), key.is_some()),
                ("OPENAI_API_KEY", true),
                "{endpoint}"
            );
        }
        // Customer-run names under azure.com, look-alikes and local proxies need the Azure key.
        for endpoint in [
            "https://public.cloudapp.azure.com",
            "https://public.azurewebsites.net",
            "https://openai.azure.com",
            "https://public.openai.azure.com.public.invalid",
            "https://publicopenai.azure.com",
            "https://public.invalid",
            "http://127.0.0.1:8080",
            "https://public.openai.azure.com.",
            "https://public.openai.azure.com@public.invalid",
            "https://public%2Eopenai.azure.com.public.invalid",
            "https://public.invalid/public.openai.azure.com",
            "https://public.invalid/?host=public.openai.azure.com",
            "https://public.invalid#.openai.azure.com",
            "https://[::1]",
            "https://public.openai.azure.cn",
            "https://public.cognitiveservices.azure.cn",
        ] {
            let (route, key) = route(endpoint, &openai);
            assert_eq!(
                (route.key_source.as_str(), key.is_some()),
                ("AZURE_OPENAI_API_KEY", false),
                "{endpoint}"
            );
            assert_eq!(
                missing_key_message(&route).split(". Set").next(),
                Some(
                    "Missing AZURE_OPENAI_API_KEY (OPENAI_API_KEY is sent only to Microsoft-run Azure OpenAI hosts)"
                ),
                "{endpoint}"
            );
            let both = Environment::from([
                ("OPENAI_API_KEY".into(), "PUBLIC-fixture".into()),
                ("AZURE_OPENAI_API_KEY".into(), "PUBLIC-azure-fixture".into()),
            ]);
            let (route, key) = super::resolve_route(
                "gpt-6-astra",
                &json!({"azure": {"endpoint": endpoint, "deployment": "d"}}),
                &both,
            )
            .unwrap();
            assert_eq!(
                (route.key_source.as_str(), key.is_some()),
                ("AZURE_OPENAI_API_KEY", true),
                "{endpoint}"
            );
        }
        let (route, _) = route("https://public.openai.azure.com", &Environment::new());
        assert!(
            missing_key_message(&route)
                .starts_with("Missing AZURE_OPENAI_API_KEY (or OPENAI_API_KEY)."),
            "{}",
            missing_key_message(&route)
        );
    }
}
#[cfg(test)]
#[path = "providers_coverage_tests.rs"]
mod coverage_tests;
