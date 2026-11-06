use std::time::Duration;

use anyhow::{Context, Result};

/// 上游请求的统一 HTTP 客户端策略。
///
/// 默认 **不使用** 系统代理：0.1 的 Node 实现本身就不读 http_proxy，
/// 而且环境里残留的代理变量会把发往 127.0.0.1 的测试请求也劫持掉。
/// 需要代理时显式设置 `CCMS_PROXY=http://127.0.0.1:7890`。
pub fn upstream(timeout: Option<Duration>, connect: Option<Duration>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(connect) = connect {
        builder = builder.connect_timeout(connect);
    }

    builder = match std::env::var("CCMS_PROXY") {
        Ok(proxy) if !proxy.trim().is_empty() => {
            let proxy = reqwest::Proxy::all(proxy.trim())
                .with_context(|| format!("CCMS_PROXY 不是合法代理地址: {proxy}"))?;
            builder.proxy(proxy)
        }
        _ => builder.no_proxy(),
    };

    builder.build().context("构建 HTTP 客户端失败")
}

/// 本机 router 的健康检查 / 探活，永远绕过代理。
pub fn local() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(1500))
        .build()?)
}
