use std::time::Duration;

use anyhow::{Context, Result};

/// 上游请求的统一 HTTP 客户端策略。
/// 需要代理时显式设置 `ALKANET_PROXY=http://127.0.0.1:7890`（兼容 `CCMS_PROXY`）。
pub fn upstream(timeout: Option<Duration>, connect: Option<Duration>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(connect) = connect {
        builder = builder.connect_timeout(connect);
    }

    let proxy = std::env::var("ALKANET_PROXY").or_else(|_| std::env::var("CCMS_PROXY"));
    builder = match proxy {
        Ok(proxy) if !proxy.trim().is_empty() => {
            let proxy = reqwest::Proxy::all(proxy.trim())
                .with_context(|| format!("ALKANET_PROXY 不是合法代理地址: {proxy}"))?;
            builder.proxy(proxy)
        }
        _ => builder.no_proxy(),
    };

    builder.build().context("构建 HTTP 客户端失败")
}
