//! 端到端验证：真实二进制 + mock 上游。
//! 覆盖：档位路由、密钥池重试、模型改写、配置热加载、SSE 统计、count_tokens 兜底、/v1/models。

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_ccms");

struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("addr").port()
}

async fn mock_upstream(port: u16) {
    let app = Router::new()
        .route("/v1/messages", post(mock_messages))
        .route(
            "/v1/messages/count_tokens",
            post(|| async {
                (axum::http::StatusCode::NOT_FOUND, "not implemented").into_response()
            }),
        )
        .with_state(());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("mock bind");
    axum::serve(listener, app).await.expect("mock serve");
}

async fn mock_messages(
    headers: HeaderMap,
    State(_): State<()>,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    // 第一把 key 固定 401，用来验证换 key 重试。
    if auth.contains("sk-bad") {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(
                json!({"type":"error","error":{"type":"authentication_error","message":"bad key"}}),
            ),
        )
            .into_response();
    }

    if body["stream"] == json!(true) {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":7,\"cache_read_input_tokens\":2}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"ok\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":4}}\n\n",
        );
        return Response::builder()
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from(sse))
            .unwrap();
    }

    Json(json!({
        "id": "msg_mock",
        "type": "message",
        "role": "assistant",
        "model": body["model"],
        "content": [{"type":"text","text":"ok"}],
        "usage": {"input_tokens": 11, "output_tokens": 5, "cache_creation_input_tokens": 3}
    }))
    .into_response()
}

async fn wait_health(port: u16) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(600))
        .build()
        .expect("client");
    for _ in 0..120 {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/ccms/health"))
            .send()
            .await
        {
            if response.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("router 未在超时内启动");
}

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_router() {
    let home = std::env::temp_dir().join(format!("ccms-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("mkdir");

    let upstream_port = free_port();
    let router_port = free_port();

    tokio::spawn(mock_upstream(upstream_port));
    tokio::time::sleep(Duration::from_millis(300)).await;

    std::fs::write(
        home.join("providers.json"),
        json!({
            "kimi": {
                "label": "Mock",
                "base_url": format!("http://127.0.0.1:{upstream_port}"),
                "key_env": "KIMI_API_KEY",
                "default_model": "mock-model",
                "models": ["mock-model"]
            }
        })
        .to_string(),
    )
    .expect("providers");

    std::fs::write(
        home.join("routes.json"),
        json!({
            "listen": format!("127.0.0.1:{router_port}"),
            "default": "sonnet",
            "effort": "high",
            "retry": { "attempts": 3, "backoff_ms": 10, "response_timeout_ms": 5000, "connect_timeout_ms": 2000, "retry_on": [401, 429, 500, 502, 503, 504] },
            "routes": {
                "opus": { "provider": "kimi", "model": "mock-model" },
                "sonnet": { "provider": "kimi", "model": "mock-model" },
                "haiku": { "provider": "kimi", "model": "mock-model" },
                "fable": { "provider": "kimi", "model": "mock-model" },
                "subagent": { "provider": "kimi", "model": "mock-model" }
            }
        })
        .to_string(),
    )
    .expect("routes");

    // 两把 key：第一把会被上游拒绝，验证换 key + 重试。
    let status = Command::new(BIN)
        .args(["key", "kimi", "sk-bad"])
        .env("CCMS_HOME", &home)
        .stdout(Stdio::null())
        .status()
        .expect("run key");
    assert!(status.success());

    let status = Command::new(BIN)
        .args(["key", "kimi", "sk-good"])
        .env("CCMS_HOME", &home)
        .stdout(Stdio::null())
        .status()
        .expect("run key");
    assert!(status.success());

    let child = Command::new(BIN)
        .args(["router", "run"])
        .env("CCMS_HOME", &home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn router");
    let _guard = Guard(child);

    wait_health(router_port).await;

    let base = format!("http://127.0.0.1:{router_port}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");

    // 1) 档位路由 + 密钥池重试（第一把 key 401，第二把成功）
    let response = client
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "ccms-router-local")
        .json(&json!({
            "model": "ccms-opus",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}]
        }))
        .send()
        .await
        .expect("post");
    if response.status() != 200 {
        let log = std::fs::read_to_string(home.join("router.log")).unwrap_or_default();
        panic!("应换 key 后成功，实际 {}：\n{log}", response.status());
    }

    // 2) 模型名里带 opus 也可路由，且模型被改写成路由里的真实 ID
    let echoed: Value = client
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "ccms-router-local")
        .json(&json!({"model": "claude-opus-4-1", "messages": []}))
        .send()
        .await
        .expect("post")
        .json()
        .await
        .expect("json");
    assert_eq!(echoed["model"], json!("mock-model"), "模型未被改写");

    // 3) 热加载：改 routes.json 后无需重启 router
    tokio::time::sleep(Duration::from_millis(50)).await;
    let routes_path = home.join("routes.json");
    let mut hot: Value =
        serde_json::from_str(&std::fs::read_to_string(&routes_path).expect("read routes"))
            .expect("routes json");
    hot["routes"]["sonnet"]["model"] = json!("mock-model-2");
    std::fs::write(&routes_path, hot.to_string()).expect("rewrite routes");
    let echoed: Value = client
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "ccms-router-local")
        .json(&json!({"model": "ccms-sonnet", "messages": []}))
        .send()
        .await
        .expect("post")
        .json()
        .await
        .expect("json");
    assert_eq!(echoed["model"], json!("mock-model-2"), "热加载未生效");

    // 4) SSE 透传
    let streamed = client
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", "ccms-router-local")
        .json(&json!({"model": "ccms-sonnet", "stream": true, "messages": []}))
        .send()
        .await
        .expect("post")
        .text()
        .await
        .expect("text");
    assert!(streamed.contains("message_delta"), "SSE 内容应原样透传");

    // 5) count_tokens 兜底
    let counted: Value = client
        .post(format!("{base}/v1/messages/count_tokens"))
        .json(&json!({"model": "ccms-haiku", "messages": [{"role":"user","content":"hello"}]}))
        .send()
        .await
        .expect("post")
        .json()
        .await
        .expect("json");
    assert!(counted["input_tokens"].as_u64().unwrap_or(0) > 0);

    // 6) /v1/models 只暴露哨兵
    let models: Value = client
        .get(format!("{base}/v1/models"))
        .send()
        .await
        .expect("models")
        .json()
        .await
        .expect("json");
    let ids: Vec<&str> = models["data"]
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or(""))
        .collect();
    assert!(ids.contains(&"ccms-opus"));
    assert!(ids.contains(&"ccms-subagent"));

    // 7) 统计按 provider/model 记账，且记录了重试
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stats: Value = serde_json::from_str(
        &std::fs::read_to_string(home.join("stats.json")).expect("stats 文件"),
    )
    .expect("stats json");
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let day = &stats["daily"][&today];
    assert!(day["total"]["requests"].as_u64().unwrap_or(0) >= 4);
    assert!(
        day["models"]["kimi/mock-model"]["requests"]
            .as_u64()
            .unwrap_or(0)
            >= 2
    );
    assert!(
        day["models"]["kimi/mock-model-2"]["requests"]
            .as_u64()
            .unwrap_or(0)
            >= 2,
        "热加载后的新模型应单独记账"
    );
    assert!(day["total"]["retries"].as_u64().unwrap_or(0) >= 1);
    assert!(day["total"]["output_tokens"].as_u64().unwrap_or(0) >= 9);
    assert!(day["tiers"]["opus"]["requests"].as_u64().unwrap_or(0) >= 1);
}
