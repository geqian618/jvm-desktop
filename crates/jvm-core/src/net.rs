//! HTTP 客户端构建（含代理设置）。

use std::future::Future;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::store::Settings;

/// 按设置构建 reqwest 客户端。
///
/// 不设置总超时——下载动辄数百 MB，整体超时无意义；连接超时 30s，
/// 下载器的分块重试负责处理读取停滞。
/// TLS 根证书使用系统证书库（rustls-native-certs）：Windows 上企业根/
/// 安全软件根安装到系统存储，浏览器与 Java 均信任它们，仅用内置
/// webpki 根会在被 HTTPS 拦截的机器上全部失败。
pub fn build_client(settings: &Settings) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(concat!("jvm/", env!("CARGO_PKG_VERSION")));
    let proxy_url = settings.proxy.trim();
    if !proxy_url.is_empty() {
        let proxy = reqwest::Proxy::all(proxy_url)?;
        builder = builder.proxy(proxy);
    }
    Ok(builder.build()?)
}

/// 元数据/详情类 API 请求的响应超时。
///
/// [`build_client`] 故意不设总超时（下载动辄数百 MB），connect_timeout
/// 也只覆盖 TCP 连接建立——服务器"收下连接却不回响应头"的半死连接会让
/// 裸 `.send().await` 永久挂起：GUI 的发行版/包列表与安装前的直链解析
/// 卡死且无法取消。下载分块有自己的 timeout 包装，client 级别不能设总
/// 超时（那会把下载的响应体一并限时），元数据类请求（响应都很小）统一
/// 经 [`api_send`]/[`api_body`] 限时。
pub const API_TIMEOUT: Duration = Duration::from_secs(30);

/// 带响应头超时的请求发送：数据源元数据类请求统一走这里。
pub async fn api_send(req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    tokio::time::timeout(API_TIMEOUT, req.send())
        .await
        .map_err(|_| Error::msg(format!("数据源请求超时（{API_TIMEOUT:?} 内未收到响应头）")))?
        .map_err(Error::from)
}

/// 带超时的响应体读取：`resp.json()` / `resp.text()` 等未来值整体限时，
/// 防止响应头已回但响应体停滞的连接挂住调用方。
pub async fn api_body<T, F>(fut: F) -> Result<T>
where
    F: Future<Output = reqwest::Result<T>>,
{
    tokio::time::timeout(API_TIMEOUT, fut)
        .await
        .map_err(|_| Error::msg("数据源响应体读取超时"))?
        .map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_builds_without_proxy() {
        assert!(build_client(&Settings::default()).is_ok());
    }

    #[test]
    fn client_builds_with_valid_proxy() {
        let mut s = Settings {
            proxy: "http://127.0.0.1:7890".into(),
            ..Settings::default()
        };
        assert!(build_client(&s).is_ok());
        // 前后空白应被 trim
        s.proxy = "  http://127.0.0.1:7890  ".into();
        assert!(build_client(&s).is_ok());
    }

    #[test]
    fn client_rejects_invalid_proxy() {
        let s = Settings {
            proxy: "not a proxy url".into(),
            ..Settings::default()
        };
        assert!(build_client(&s).is_err());
    }
}
