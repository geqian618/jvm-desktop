//! HTTP 客户端构建（含代理设置）。

use std::time::Duration;

use crate::error::Result;
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
