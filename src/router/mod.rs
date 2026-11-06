mod sse;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use axum::body::{to_bytes, Body};
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use bytes::Bytes;
use chrono::Utc;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_stream::wrappers::ReceiverStream;

use crate::config::{self, Provider, Providers, RoutesConfig, TIERS};
use crate::fsutil;
use crate::keystore::{self, Vault};
use crate::paths::Paths;
use crate::stats::{self, Usage};

const MAX_BODY: usize = 128 * 1024 * 1024;

type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    fs::metadata(path)
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())))
}

pub fn log_line(path: &Path, line: &str) {
    let ts = Utc::now().format("%Y-%m-%d %H:%M:%S");
    let entry = format!("[{ts}] {line}\n");
    use std::io::Write;
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(entry.as_bytes());
    }
}

/// 路由与 provider 按 mtime 热加载：改完配置无需重启 router。
struct ConfigCache {
    routes: Arc<RoutesConfig>,
    providers: Arc<Providers>,
    routes_stamp: Stamp,
    providers_stamp: Stamp,
}

impl ConfigCache {
    fn load(paths: &Paths) -> Result<Self> {
        Ok(Self {
            routes: Arc::new(config::load_routes(&paths.routes)?),
            providers: Arc::new(config::load_providers(&paths.providers)?),
            routes_stamp: stamp(&paths.routes),
            providers_stamp: stamp(&paths.providers),
        })
    }

    fn refresh(&mut self, paths: &Paths) -> (Arc<RoutesConfig>, Arc<Providers>) {
        let routes_stamp = stamp(&paths.routes);
        if routes_stamp != self.routes_stamp {
            match config::load_routes(&paths.routes) {
                Ok(cfg) => {
                    self.routes = Arc::new(cfg);
                    self.routes_stamp = routes_stamp;
                    log_line(&paths.log, "配置热加载: routes.json");
                }
                Err(err) => log_line(
                    &paths.log,
                    &format!("routes.json 解析失败，继续使用旧配置: {err:#}"),
                ),
            }
        }

        let providers_stamp = stamp(&paths.providers);
        if providers_stamp != self.providers_stamp {
            match config::load_providers(&paths.providers) {
                Ok(list) => {
                    self.providers = Arc::new(list);
                    self.providers_stamp = providers_stamp;
                    log_line(&paths.log, "配置热加载: providers.json");
                }
                Err(err) => log_line(
                    &paths.log,
                    &format!("providers.json 解析失败，继续使用旧配置: {err:#}"),
                ),
            }
        }

        (self.routes.clone(), self.providers.clone())
    }
}

/// 解密后的密钥池缓存：mtime 变了才重新解密，口令模式不会每请求都跑 scrypt。
struct KeysCache {
    map: Arc<BTreeMap<String, String>>,
    enc_stamp: Stamp,
    key_stamp: Stamp,
    loaded: bool,
}

impl KeysCache {
    fn empty() -> Self {
        Self {
            map: Arc::new(BTreeMap::new()),
            enc_stamp: None,
            key_stamp: None,
            loaded: false,
        }
    }

    fn ensure(&mut self, vault: &Vault, log: &Path) {
        let enc_stamp = stamp(&vault.enc);
        let key_stamp = stamp(&vault.keyfile);
        if self.loaded && enc_stamp == self.enc_stamp && key_stamp == self.key_stamp {
            return;
        }
        match vault.load() {
            Ok(map) => self.map = Arc::new(map),
            Err(err) => {
                log_line(log, &format!("密钥库读取失败: {err:#}"));
                self.map = Arc::new(BTreeMap::new());
            }
        }
        self.enc_stamp = enc_stamp;
        self.key_stamp = key_stamp;
        self.loaded = true;
    }
}

#[derive(Clone)]
pub struct AppState {
    paths: Paths,
    client: reqwest::Client,
    started: Instant,
    config: Arc<Mutex<ConfigCache>>,
    keys: Arc<Mutex<KeysCache>>,
    cursor: Arc<Mutex<HashMap<String, usize>>>,
    stats_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AppState {
    fn new(paths: Paths, client: reqwest::Client, config: ConfigCache) -> Self {
        Self {
            paths,
            client,
            started: Instant::now(),
            config: Arc::new(Mutex::new(config)),
            keys: Arc::new(Mutex::new(KeysCache::empty())),
            cursor: Arc::new(Mutex::new(HashMap::new())),
            stats_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn snapshot(&self) -> (Arc<RoutesConfig>, Arc<Providers>) {
        let mut cache = self.config.lock().unwrap_or_else(|err| err.into_inner());
        cache.refresh(&self.paths)
    }

    fn keys_for(&self, provider: &Provider) -> Vec<String> {
        let vault = Vault::new(&self.paths);
        let map = {
            let mut cache = self.keys.lock().unwrap_or_else(|err| err.into_inner());
            cache.ensure(&vault, &self.paths.log);
            cache.map.clone()
        };
        keystore::keys_for(&map, provider)
    }

    /// 每个 provider 一个轮询游标，保证多 key 时均匀分摊。
    fn next_key_index(&self, provider: &str, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        let mut cursor = self.cursor.lock().unwrap_or_else(|err| err.into_inner());
        let current = *cursor.get(provider).unwrap_or(&0);
        cursor.insert(provider.to_string(), (current + 1) % len);
        current % len
    }

    fn log(&self, line: &str) {
        log_line(&self.paths.log, line);
    }

    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        tier: &str,
        provider: &str,
        model: &str,
        usage: Option<Usage>,
        retries: u32,
        error: bool,
        duration_ms: u64,
    ) {
        let _guard = self.stats_lock.lock().await;
        if let Err(err) = stats::record(
            &self.paths.stats,
            tier,
            provider,
            model,
            usage,
            u64::from(retries),
            error,
            duration_ms,
        ) {
            self.log(&format!("stats error: {err:#}"));
        }
    }
}

pub async fn run(paths: Paths) -> Result<()> {
    config::ensure_defaults(&paths)?;
    let cfg = config::load_routes(&paths.routes)?;
    let listen = cfg.listen.clone();
    let addr: SocketAddr = listen
        .parse()
        .with_context(|| format!("listen 地址非法: {listen}"))?;
    if !addr.ip().is_loopback() {
        bail!("listen 必须是本机回环地址（当前 {listen}），不要对外暴露密钥路由器");
    }

    let client = crate::http::upstream(
        None,
        Some(Duration::from_millis(
            cfg.retry.connect_timeout_ms.max(1000),
        )),
    )?;

    let vault = Vault::new(&paths);
    if !vault.exists() {
        match vault.migrate_legacy() {
            Ok(Some(count)) => println!("ccms: 已将 {count} 个明文密钥迁移进 keys.enc"),
            Ok(None) => {}
            Err(err) => eprintln!("ccms: 明文密钥迁移失败: {err:#}"),
        }
    }

    let cache = ConfigCache::load(&paths)?;
    let state = AppState::new(paths.clone(), client, cache);

    let app = Router::new()
        .route("/ccms/health", get(health))
        .route("/v1/models", get(models))
        .fallback(proxy)
        .layer(DefaultBodyLimit::disable())
        .with_state(state);

    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            println!("ccms router: {listen} 已被占用（可能已经在运行）");
            return Ok(());
        }
        Err(err) => {
            return Err(err).with_context(|| format!("无法监听 {listen}"));
        }
    };

    let pid = std::process::id();
    fsutil::write_atomic(&paths.pid, format!("{pid}\n").as_bytes(), 0o644)?;
    println!(
        "ccms router v{} listening on http://{listen}  retry={}x",
        env!("CARGO_PKG_VERSION"),
        cfg.retry.attempts
    );
    log_line(
        &paths.log,
        &format!(
            "router start pid={pid} listen={listen} retry={}",
            cfg.retry.attempts
        ),
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    let _ = fs::remove_file(&paths.pid);
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(sig) => sig,
            Err(_) => return,
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(sig) => sig,
            Err(_) => return,
        };
        tokio::select! {
            _ = term.recv() => {},
            _ = int.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    eprintln!("ccms router: 收到退出信号");
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let (routes, providers) = state.snapshot();
    let mut keys = serde_json::Map::new();
    for (name, provider) in providers.iter() {
        keys.insert(name.clone(), json!(state.keys_for(provider).len()));
    }
    Json(json!({
        "ok": true,
        "service": "ccms",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": state.started.elapsed().as_secs(),
        "listen": routes.listen,
        "default": routes.default,
        "routes": routes.routes,
        "retry": routes.retry,
        "keys": keys,
    }))
}

async fn models(State(state): State<AppState>) -> Json<Value> {
    let (routes, _) = state.snapshot();
    let created = Utc::now().to_rfc3339();
    let data: Vec<Value> = routes
        .routes
        .iter()
        .map(|(tier, route)| {
            json!({
                "id": format!("ccms-{tier}"),
                "type": "model",
                "display_name": format!("ccms {tier} → {}/{}", route.provider, route.model),
                "created_at": created,
            })
        })
        .collect();
    let first = data
        .first()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let last = data
        .last()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Json(json!({
        "data": data,
        "has_more": false,
        "first_id": first,
        "last_id": last,
    }))
}

fn skip_request_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "authorization"
            | "x-api-key"
            | "content-length"
            | "connection"
            | "keep-alive"
            | "proxy-connection"
            | "transfer-encoding"
            | "accept-encoding"
    )
}

fn skip_response_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "content-length" | "transfer-encoding" | "connection" | "keep-alive"
    )
}

fn json_error(status: StatusCode, kind: &str, message: &str) -> Response {
    let body = json!({
        "type": "error",
        "error": { "type": kind, "message": message },
    });
    let mut response = Response::new(Body::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// 档位解析：先认 `ccms-<tier>` 哨兵名，再认模型名里的档位关键字，最后落到默认档位。
pub fn resolve_tier(model: &str, cfg: &RoutesConfig) -> String {
    let lower = model.to_ascii_lowercase();
    let bare = lower.strip_prefix("ccms-").unwrap_or(&lower);
    if cfg.routes.contains_key(bare) {
        return bare.to_string();
    }
    for tier in TIERS {
        if lower.contains(tier) {
            return tier.to_string();
        }
    }
    cfg.default.clone()
}

fn extract_usage(value: &Value) -> Option<Usage> {
    let usage = value.get("usage")?;
    Some(Usage {
        input: usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output: usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_create: usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
}

async fn proxy(State(state): State<AppState>, request: Request) -> Response {
    let started = Instant::now();
    let (parts, body) = request.into_parts();

    let raw_body = match to_bytes(body, MAX_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return json_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_request_error",
                "ccms: 请求体过大",
            )
        }
    };

    let (routes, providers) = state.snapshot();
    let mut json_body: Option<Value> = if raw_body.is_empty() {
        None
    } else {
        serde_json::from_slice(&raw_body).ok()
    };
    let requested_model = json_body
        .as_ref()
        .and_then(|value| value.get("model"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let tier = resolve_tier(&requested_model, &routes);

    let Some(route) = routes.routes.get(&tier).cloned() else {
        state.log(&format!("{tier} 无路由配置 → 503"));
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            &format!(
                "ccms: 档位 {tier} 没有配置路由，编辑 {}",
                state.paths.routes.display()
            ),
        );
    };

    let Some(provider) = providers.get(&route.provider).cloned() else {
        state.log(&format!("{tier} provider {} 不存在 → 503", route.provider));
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            &format!(
                "ccms: provider {} 未在 providers.json 中定义",
                route.provider
            ),
        );
    };

    let keys = state.keys_for(&provider);
    if keys.is_empty() {
        state.log(&format!("{tier} {} 密钥池为空 → 401", route.provider));
        return json_error(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            &format!(
                "ccms: {} 没有可用 key，运行 ccms key {} sk-xxx",
                route.provider, route.provider
            ),
        );
    }

    if let Some(object) = json_body.as_mut().and_then(|value| value.as_object_mut()) {
        if object.contains_key("model") {
            object.insert("model".into(), Value::String(route.model.clone()));
        }
    }
    let payload: Bytes = match &json_body {
        Some(value) => Bytes::from(serde_json::to_vec(value).unwrap_or_else(|_| raw_body.to_vec())),
        None => raw_body.clone(),
    };

    let base = provider.base_url.trim_end_matches('/');
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let url = format!("{base}{path_and_query}");
    let is_count_tokens = parts.uri.path().ends_with("/count_tokens");

    let headers: Vec<(HeaderName, HeaderValue)> = parts
        .headers
        .iter()
        .filter(|(name, _)| !skip_request_header(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let has_anthropic_version = headers
        .iter()
        .any(|(name, _)| name.as_str() == "anthropic-version");

    let max_attempts = routes.retry.attempts.max(1);
    let start_index = state.next_key_index(&route.provider, keys.len());
    let mut attempt: u32 = 0;
    let mut last_error = String::new();
    let mut upstream: Option<reqwest::Response> = None;
    let mut used_key = start_index;

    while attempt < max_attempts {
        attempt += 1;
        let key_index = (start_index + (attempt as usize - 1)) % keys.len();
        used_key = key_index;

        let mut builder = state
            .client
            .request(parts.method.clone(), &url)
            .body(payload.clone());
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }
        if !has_anthropic_version {
            builder = builder.header("anthropic-version", "2023-06-01");
        }
        builder = if provider.uses_x_api_key() {
            builder.header("x-api-key", &keys[key_index])
        } else {
            builder.bearer_auth(&keys[key_index])
        };

        let timeout = Duration::from_millis(routes.retry.response_timeout_ms.max(5_000));
        match tokio::time::timeout(timeout, builder.send()).await {
            Ok(Ok(response)) => {
                let status = response.status().as_u16();
                let retryable = routes.retry.retry_on.contains(&status);
                if retryable && attempt < max_attempts {
                    state.log(&format!(
                        "{tier} {}/{} HTTP {status} key#{}/{} → 重试 {}/{max_attempts}",
                        route.provider,
                        route.model,
                        key_index + 1,
                        keys.len(),
                        attempt
                    ));
                    let delay = if status == 401 {
                        50
                    } else {
                        (routes.retry.backoff_ms * u64::from(attempt)).min(5_000)
                    };
                    drop(response);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                upstream = Some(response);
                break;
            }
            Ok(Err(err)) => {
                last_error = err.to_string();
                if attempt < max_attempts {
                    let delay = (routes.retry.backoff_ms * u64::from(attempt)).min(5_000);
                    state.log(&format!(
                        "{tier} {}/{} ERR {last_error} key#{}/{} → 重试 {}/{max_attempts} in {delay}ms",
                        route.provider,
                        route.model,
                        key_index + 1,
                        keys.len(),
                        attempt
                    ));
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
            Err(_) => {
                last_error = format!("等待响应超时 {}s", routes.retry.response_timeout_ms / 1000);
                if attempt < max_attempts {
                    let delay = (routes.retry.backoff_ms * u64::from(attempt)).min(5_000);
                    state.log(&format!(
                        "{tier} {}/{} {last_error} → 重试 {}/{max_attempts}",
                        route.provider, route.model, attempt
                    ));
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
    }

    let Some(response) = upstream else {
        let elapsed = started.elapsed().as_millis() as u64;
        state
            .record(
                &tier,
                &route.provider,
                &route.model,
                None,
                attempt.saturating_sub(1),
                true,
                elapsed,
            )
            .await;
        state.log(&format!(
            "{tier} {}/{} 全部重试失败: {last_error}",
            route.provider, route.model
        ));
        return json_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            &format!("ccms: 连不上 {}（{last_error}）", route.provider),
        );
    };

    let status = response.status();

    if is_count_tokens
        && (status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED)
    {
        let estimate = ((payload.len() as f64) / 4.0).ceil().max(1.0) as u64;
        state.log(&format!(
            "{tier} {}/{} count_tokens {} → 本地估算 {estimate}",
            route.provider,
            route.model,
            status.as_u16()
        ));
        return Json(json!({ "input_tokens": estimate })).into_response();
    }

    let mut out_headers = HeaderMap::new();
    for (name, value) in response.headers() {
        if !skip_response_header(name) {
            out_headers.insert(name.clone(), value.clone());
        }
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let is_sse = content_type.contains("text/event-stream");

    if !is_sse {
        let body = match tokio::time::timeout(
            Duration::from_millis(routes.retry.response_timeout_ms.max(5_000)),
            response.bytes(),
        )
        .await
        {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(err)) => {
                let elapsed = started.elapsed().as_millis() as u64;
                state
                    .record(
                        &tier,
                        &route.provider,
                        &route.model,
                        None,
                        attempt.saturating_sub(1),
                        true,
                        elapsed,
                    )
                    .await;
                state.log(&format!("{tier} 读取响应失败: {err}"));
                return json_error(
                    StatusCode::BAD_GATEWAY,
                    "api_error",
                    &format!("ccms: 读取上游响应失败（{err}）"),
                );
            }
            Err(_) => {
                let elapsed = started.elapsed().as_millis() as u64;
                state
                    .record(
                        &tier,
                        &route.provider,
                        &route.model,
                        None,
                        attempt.saturating_sub(1),
                        true,
                        elapsed,
                    )
                    .await;
                return json_error(
                    StatusCode::GATEWAY_TIMEOUT,
                    "api_error",
                    "ccms: 等待上游响应超时",
                );
            }
        };

        let usage = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|value| extract_usage(&value));
        let elapsed = started.elapsed().as_millis() as u64;
        state
            .record(
                &tier,
                &route.provider,
                &route.model,
                usage,
                attempt.saturating_sub(1),
                status.as_u16() >= 400,
                elapsed,
            )
            .await;
        state.log(&format!(
            "{tier} {}/{} {} {elapsed}ms key#{}/{}",
            route.provider,
            route.model,
            status.as_u16(),
            used_key + 1,
            keys.len()
        ));

        let mut result = Response::new(Body::from(body));
        *result.status_mut() = status;
        *result.headers_mut() = out_headers;
        return result;
    }

    // 流式响应：边转发边解析 usage；一旦开始 pipe 就不再重连，避免重复输出。
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let task_state = state.clone();
    let task_tier = tier.clone();
    let task_provider = route.provider.clone();
    let task_model = route.model.clone();
    let retries = attempt.saturating_sub(1);
    let key_label = format!("{}/{}", used_key + 1, keys.len());
    let status_code = status.as_u16();

    tokio::spawn(async move {
        let mut parser = sse::Parser::default();
        let mut stream = response.bytes_stream();
        let mut failed = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => {
                    parser.feed(&chunk);
                    if tx.send(Ok(chunk)).await.is_err() {
                        break;
                    }
                }
                Err(err) => {
                    failed = true;
                    let _ = tx.send(Err(std::io::Error::other(err.to_string()))).await;
                    break;
                }
            }
        }
        let elapsed = started.elapsed().as_millis() as u64;
        task_state
            .record(
                &task_tier,
                &task_provider,
                &task_model,
                parser.usage(),
                retries,
                failed || status_code >= 400,
                elapsed,
            )
            .await;
        task_state.log(&format!(
            "{task_tier} {task_provider}/{task_model} {status_code} {elapsed}ms key#{key_label}"
        ));
    });

    let mut result = Response::new(Body::from_stream(ReceiverStream::new(rx)));
    *result.status_mut() = status;
    *result.headers_mut() = out_headers;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_default() -> RoutesConfig {
        serde_json::from_str::<RoutesConfig>(
            r#"{"default":"sonnet","routes":{"sonnet":{"provider":"p","model":"m"},"opus":{"provider":"p","model":"o"}}}"#,
        )
        .unwrap()
    }

    #[test]
    fn tier_resolution_prefers_sentinel() {
        let cfg = config_with_default();
        assert_eq!(resolve_tier("ccms-opus", &cfg), "opus");
        assert_eq!(resolve_tier("claude-opus-4-1", &cfg), "opus");
        assert_eq!(resolve_tier("ccms-haiku", &cfg), "haiku");
        assert_eq!(resolve_tier("", &cfg), "sonnet");
        assert_eq!(resolve_tier("unknown-model", &cfg), "sonnet");
    }

    #[test]
    fn usage_extraction_is_lenient() {
        let value: Value = serde_json::json!({
            "usage": { "input_tokens": 5, "output_tokens": 9, "cache_read_input_tokens": 2 }
        });
        let usage = extract_usage(&value).unwrap();
        assert_eq!(usage.input, 5);
        assert_eq!(usage.output, 9);
        assert_eq!(usage.cache_read, 2);
        assert_eq!(usage.cache_create, 0);
        assert!(extract_usage(&serde_json::json!({})).is_none());
    }
}
