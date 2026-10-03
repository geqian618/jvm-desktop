//! foojay Disco API v3.0 客户端。
//!
//! 实测（2026-10）API 行为要点：
//! - 基址为 `https://api.foojay.io/disco/v3.0/`（注意是 `v3.0`，`v3` 会 404）；
//! - `/packages` 不带 version 参数时返回该发行商当前平台归档格式的全部
//!   GA 包（「所有版本」页一次拉全量，筛选在本地做）；
//! - 带 `version=<feature>&latest=available` 只返回该主版本最新 GA；
//!   `version=<精确版本>` 精确命中；
//! - 列表响应不含下载直链与校验和，需再请求 `/ids/{id}` 获取
//!   `direct_download_uri` / `checksum`（sha256 hex）；
//! - `/major_versions` 返回各主版本的 LTS 标记、维护状态与全部 GA 版本串。
//! - 匿名请求可用；如部署要求鉴权，用 `X-API-Key` 头。

use std::sync::Arc;

use serde::Deserialize;

use super::{DistributionInfo, MajorVersion, PackageDetail, PkgQuery, RegistrySource};
use crate::error::{Error, Result};
use crate::model::{Release, RemotePackage, version_feature};

#[derive(Debug, Clone)]
pub struct FoojaySource {
    base: String,
    api_key: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct FjPackage {
    id: String,
    distribution: String,
    major_version: u32,
    java_version: String,
    distribution_version: String,
    release_status: String,
    term_of_support: String,
    operating_system: String,
    architecture: String,
    archive_type: String,
    package_type: String,
    javafx_bundled: bool,
    directly_downloadable: bool,
    filename: String,
    size: Option<u64>,
    links: FjLinks,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct FjLinks {
    #[serde(default, rename = "pkg_info_uri")]
    pkg_info_uri: String,
    #[serde(default, rename = "pkg_download_redirect")]
    pkg_download_redirect: String,
}

#[derive(Deserialize)]
struct FjPackageList {
    #[serde(default)]
    result: Vec<FjPackage>,
}

#[derive(Deserialize)]
struct FjIdDetail {
    #[serde(default)]
    result: Vec<FjIdEntry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FjIdEntry {
    #[serde(rename = "direct_download_uri")]
    direct_download_uri: String,
    #[serde(rename = "checksum")]
    checksum: String,
    #[serde(rename = "checksum_uri")]
    checksum_uri: String,
}

#[derive(Debug, Deserialize)]
struct FjDistribution {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "api_parameter")]
    api_parameter: String,
    #[serde(default)]
    maintained: bool,
    #[serde(default)]
    versions: Vec<String>,
}

#[derive(Deserialize)]
struct FjDistributionList {
    #[serde(default)]
    result: Vec<FjDistribution>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FjMajorVersion {
    major_version: u32,
    term_of_support: String,
    release_status: String,
    maintained: bool,
    versions: Vec<String>,
}

#[derive(Deserialize)]
struct FjMajorVersions {
    #[serde(default)]
    result: Vec<FjMajorVersion>,
}

/// URL 匹配器（SPECS 注册表用）：全匹配兜底——空或官方地址用官方 base，
/// 其余 URL 一律视为 foojay 兼容镜像、原样对接。必须排在 SPECS 末位。
pub fn try_create(url: &str, api_key: &str, client: reqwest::Client) -> Option<super::Source> {
    let base = if url.is_empty() {
        super::FOOJAY_BASE.to_string()
    } else {
        url.to_string()
    };
    Some(Arc::new(FoojaySource::new(
        base,
        api_key.to_string(),
        client,
    )))
}

impl FoojaySource {
    pub fn new(base: String, api_key: String, client: reqwest::Client) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            api_key,
            client,
        }
    }

    /// foojay 部分部署需要 API Key；匿名可用时不发送。
    /// `path` 允许传入完整 URL（如 /ids 详情返回的绝对地址）。
    fn request(&self, path: &str, query: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        };
        let mut req = self.client.get(url).query(query);
        if !self.api_key.is_empty() {
            req = req.header("X-API-Key", &self.api_key);
        }
        req
    }

    /// 统一把非 2xx 的错误体转成可读信息（foojay 的错误在 _embedded.errors）。
    async fn error_body(resp: reqwest::Response) -> Error {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v["_embedded"]["errors"][0]["message"]
                    .as_str()
                    .or_else(|| v["message"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.chars().take(300).collect());
        Error::msg(format!("foojay {status}: {detail}"))
    }

    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let resp = match self.request(path, query).send().await {
            Ok(r) if !r.status().is_success() => return Err(Self::error_body(r).await),
            Ok(r) => r,
            Err(e) => return Err(e.into()),
        };
        Ok(resp.json::<T>().await?)
    }

    fn to_remote_package(p: &FjPackage) -> RemotePackage {
        RemotePackage {
            os: p.operating_system.clone(),
            arch: p.architecture.clone(),
            archive_type: p.archive_type.clone(),
            size: p.size,
            filename: p.filename.clone(),
            distribution_version: p.distribution_version.clone(),
            image_type: p.package_type.clone(),
            direct_url: {
                let r = p.links.pkg_download_redirect.trim();
                (!r.is_empty()).then(|| r.to_string())
            },
            info_uri: {
                let i = p.links.pkg_info_uri.trim();
                (!i.is_empty()).then(|| i.to_string())
            },
            checksum_sha256: None,
            checksum_url: None,
        }
    }

    /// 多候选时选优：优先非 JavaFX 捆绑的包。
    fn choose(packages: Vec<FjPackage>) -> Option<FjPackage> {
        let mut list = packages;
        list.sort_by_key(|p| p.javafx_bundled);
        list.first().cloned()
    }

    async fn fetch_packages(
        &self,
        q: &PkgQuery,
        version: Option<&str>,
        latest: bool,
    ) -> Result<Vec<FjPackage>> {
        let mut params: Vec<(&str, &str)> = vec![
            ("distribution", &q.vendor),
            ("operating_system", &q.os),
            ("architecture", &q.arch),
            ("archive_type", &q.archive_type),
            ("package_type", &q.image_type),
            ("directly_downloadable", "true"),
            ("release_status", "ga"),
        ];
        if let Some(v) = version {
            params.push(("version", v));
        }
        if latest {
            params.push(("latest", "available"));
        }
        let resp = match self.request("/packages", &params).send().await {
            Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
                let v = version.unwrap_or("(all)");
                return Err(Error::msg(format!(
                    "foojay 未收录该组合（{} / {v} / {}）：检查发行商拼写或镜像配置",
                    q.vendor, q.os
                )));
            }
            Ok(r) if !r.status().is_success() => return Err(Self::error_body(r).await),
            Ok(r) => r,
            Err(e) => return Err(e.into()),
        };
        let list: FjPackageList = resp.json().await?;
        Ok(list.result)
    }

    /// foojay 的 checksum 字段并非总是 sha256：liberica 等会给 40 位 SHA-1，
    /// 当成 sha256 比对必然"校验和不匹配"。只有 64 位十六进制才可采信。
    fn valid_sha256(s: &str) -> Option<String> {
        let s = s.trim().to_lowercase();
        (s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())).then_some(s)
    }

    async fn fill_detail(&self, pkg: &mut RemotePackage, fj: &FjPackage) -> Result<()> {
        if fj.links.pkg_info_uri.is_empty() {
            return Ok(());
        }
        // pkg_info_uri 在真实 API 里是绝对地址，request() 同时兼容绝对 URL 与相对路径。
        let detail = self.package_detail_by_uri(&fj.links.pkg_info_uri).await?;
        if detail.direct_url.is_some() {
            pkg.direct_url = detail.direct_url;
        }
        if detail.checksum_sha256.is_some() {
            pkg.checksum_sha256 = detail.checksum_sha256;
        }
        if detail.checksum_url.is_some() {
            pkg.checksum_url = detail.checksum_url;
        }
        Ok(())
    }

    fn to_release(fj: &FjPackage) -> Option<Release> {
        let version = if fj.java_version.is_empty() {
            fj.distribution_version.clone()
        } else {
            fj.java_version.clone()
        };
        if version.is_empty() {
            return None;
        }
        let feature = if fj.major_version > 0 {
            fj.major_version
        } else {
            version_feature(&version)?
        };
        Some(Release {
            vendor: fj.distribution.clone(),
            version,
            feature,
            term: fj.term_of_support.to_uppercase(),
            lts: fj.term_of_support.eq_ignore_ascii_case("lts"),
            package: Self::to_remote_package(fj),
        })
    }
}

/// [`RegistrySource`] 实现：把 foojay 响应归一化为通用模型。
/// 私有辅助（request/send_json/fill_detail/to_release 等）留在上方固有 impl。
#[async_trait::async_trait]
impl RegistrySource for FoojaySource {
    fn spec_name(&self) -> &'static str {
        "foojay"
    }

    /// `/ids/{id}` 详情：按 URI 直查（URI 来自列表的 pkg_info_uri），
    /// 绕开 `version=` 过滤对部分包（graalvm/dragonwell 等）命不中的问题。
    async fn package_detail_by_uri(&self, uri: &str) -> Result<PackageDetail> {
        let resp = match self.request(uri, &[]).send().await {
            Ok(r) if !r.status().is_success() => return Err(Self::error_body(r).await),
            Ok(r) => r,
            Err(e) => return Err(e.into()),
        };
        let detail: FjIdDetail = resp.json().await?;
        let Some(entry) = detail.result.first() else {
            return Ok(PackageDetail::default());
        };
        Ok(PackageDetail {
            direct_url: (!entry.direct_download_uri.trim().is_empty())
                .then(|| entry.direct_download_uri.trim().to_string()),
            checksum_sha256: Self::valid_sha256(&entry.checksum),
            checksum_url: (!entry.checksum_uri.trim().is_empty())
                .then(|| entry.checksum_uri.trim().to_string()),
        })
    }

    async fn latest_for_feature(&self, query: &PkgQuery) -> Result<Option<Release>> {
        let version = query.feature.to_string();
        let packages = self
            .fetch_packages(query, Some(&version), true)
            .await?;
        Ok(Self::choose(packages).as_ref().and_then(Self::to_release))
    }

    async fn package_for_exact(
        &self,
        query: &PkgQuery,
        version: &str,
    ) -> Result<Option<Release>> {
        let packages = self.fetch_packages(query, Some(version), false).await?;
        let Some(fj) = Self::choose(packages) else {
            return Ok(None);
        };
        let mut release = match Self::to_release(&fj) {
            Some(r) => r,
            None => return Ok(None),
        };
        self.fill_detail(&mut release.package, &fj).await?;
        Ok(Some(release))
    }

    /// 不带 version 参数：一次拉取该发行商当前平台+归档格式的全部 GA 包。
    async fn all_packages(
        &self,
        vendor: &str,
        os: &str,
        arch: &str,
        archive_type: &str,
    ) -> Result<Vec<Release>> {
        let query = PkgQuery {
            vendor: vendor.to_string(),
            feature: 0,
            os: os.to_string(),
            arch: arch.to_string(),
            archive_type: archive_type.to_string(),
            image_type: "jdk".to_string(),
        };
        let packages = self.fetch_packages(&query, None, false).await?;
        let mut releases: Vec<Release> = packages.iter().filter_map(Self::to_release).collect();
        // 同一 java_version 可能有多条（JavaFX 变体等），按版本降序去重保留首选。
        releases.sort_by(|a, b| crate::model::cmp_versions(&b.version, &a.version));
        releases.dedup_by(|a, b| {
            a.version == b.version
                && a.vendor == b.vendor
                && a.package.image_type == b.package.image_type
        });
        Ok(releases)
    }

    async fn distributions(&self) -> Result<Vec<DistributionInfo>> {
        let list: FjDistributionList = self.send_json("/distributions", &[]).await?;
        let mut out: Vec<DistributionInfo> = list
            .result
            .into_iter()
            .map(|d| DistributionInfo {
                api_name: d.api_parameter,
                name: d.name,
                maintained: d.maintained,
                versions: d.versions,
            })
            .collect();
        // 稳定排序：维护中的在前，同名按名称。
        out.sort_by(|a, b| b.maintained.cmp(&a.maintained).then(a.name.cmp(&b.name)));
        Ok(out)
    }

    async fn major_versions(&self) -> Result<Vec<MajorVersion>> {
        let list: FjMajorVersions = self.send_json("/major_versions", &[]).await?;
        let mut feats: Vec<MajorVersion> = list
            .result
            .iter()
            // 只列 GA 的主版本（EA-only 的不进下拉）。
            .filter(|m| m.release_status.eq_ignore_ascii_case("ga"))
            .map(|m| MajorVersion {
                major_version: m.major_version,
                term_of_support: m.term_of_support.to_uppercase(),
                maintained: m.maintained,
                versions: m.versions.clone(),
            })
            .collect();
        feats.sort_by_key(|m| m.major_version);
        Ok(feats)
    }

    /// 版本串来自 /major_versions（foojay 无按发行商的版本列表端点）；
    /// 个别版本串可能在该发行商的特定 os/arch 上无包，安装时会明确报错。
    async fn version_strings(&self, _vendor: &str, feature: u32) -> Result<Vec<String>> {
        let majors = self.major_versions().await?;
        Ok(majors
            .into_iter()
            .find(|m| m.major_version == feature)
            .map(|m| m.versions)
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::JdkId;
    use crate::source::FOOJAY_BASE;

    #[test]
    fn request_supports_relative_and_absolute() {
        let src = FoojaySource::new(
            format!("{FOOJAY_BASE}/"),
            String::new(),
            reqwest::Client::new(),
        );
        // 相对路径拼接到 base；绝对 URL 原样使用（/ids 详情是绝对地址）。
        assert_eq!(
            src.request("/packages", &[])
                .build()
                .unwrap()
                .url()
                .to_string(),
            "https://api.foojay.io/disco/v3.0/packages"
        );
        assert_eq!(
            src.request("https://example.com/x", &[])
                .build()
                .unwrap()
                .url()
                .to_string(),
            "https://example.com/x"
        );
    }

    #[tokio::test]
    async fn parses_real_package_list_shape() {
        // 用固定 JSON 验证反序列化字段（与 2026-10 实测响应一致）。
        let body = r#"{"result":[{"id":"c986","archive_type":"zip","distribution":"temurin",
            "major_version":17,"java_version":"17.0.20.1+1","distribution_version":"17.0.20.1",
            "jdk_version":17,"latest_build_available":true,"release_status":"ga","term_of_support":"lts",
            "operating_system":"windows","lib_c_type":"c_std_lib","architecture":"x64","fpu":"unknown",
            "package_type":"jdk","javafx_bundled":false,"directly_downloadable":true,
            "filename":"OpenJDK17U-jdk_x64_windows_hotspot_17.0.20.1_1.zip",
            "links":{"pkg_info_uri":"https://api.foojay.io/disco/v3.0/ids/c986",
                     "pkg_download_redirect":"https://api.foojay.io/disco/v3.0/ids/c986/redirect"},
            "free_use_in_production":true,"size":190817615,"feature":[]}],"message":"1 package(s) found"}"#;
        let list: FjPackageList = serde_json::from_str(body).unwrap();
        assert_eq!(list.result.len(), 1);
        let release = FoojaySource::to_release(&list.result[0]).unwrap();
        assert_eq!(
            release.id().to_string(),
            JdkId::parse("temurin@17.0.20.1+1").unwrap().to_string()
        );
        assert!(release.lts);
        assert_eq!(release.package.size, Some(190817615));
        assert_eq!(
            release.package.filename,
            "OpenJDK17U-jdk_x64_windows_hotspot_17.0.20.1_1.zip"
        );
        assert!(release.package.info_uri.is_some());
        assert!(release.package.direct_url.is_some());
    }

    #[test]
    fn prefers_non_javafx_packages() {
        let mk = |javafx: bool| FjPackage {
            javafx_bundled: javafx,
            ..Default::default()
        };
        let chosen = FoojaySource::choose(vec![mk(true), mk(false)]).unwrap();
        assert!(!chosen.javafx_bundled);
    }

    #[test]
    fn parses_distribution_and_major_shapes() {
        let dists: FjDistributionList = serde_json::from_str(
            r#"{"result":[{"name":"AOJ","api_parameter":"aoj","maintained":true,"versions":["16"]}]}"#,
        )
        .unwrap();
        assert_eq!(dists.result[0].api_parameter, "aoj");

        let majors: FjMajorVersions = serde_json::from_str(
            r#"{"result":[{"major_version":17,"term_of_support":"LTS","release_status":"ga","maintained":true,"versions":["17.0.12+7"]}]}"#,
        )
        .unwrap();
        assert_eq!(majors.result[0].major_version, 17);
    }

    #[test]
    fn valid_sha256_rejects_sha1_and_garbage() {
        // liberica 等发行商的 checksum 字段可能是 40 位 SHA-1，不可当作 sha256
        let sha1 = "a123456789abcdef0123456789abcdef01234567".to_string();
        assert_eq!(FoojaySource::valid_sha256(&sha1), None);

        let sha256 = "e53a79c3c3d86865bd7e787903884331068e71321714ffd44f145785affc7cb0".to_string();
        assert_eq!(FoojaySource::valid_sha256(&sha256), Some(sha256.clone()));
        // 大写归一化为小写
        let upper = sha256.to_uppercase();
        assert_eq!(FoojaySource::valid_sha256(&upper), Some(sha256));
        assert_eq!(FoojaySource::valid_sha256("short"), None);
        assert_eq!(FoojaySource::valid_sha256(&"z".repeat(64)), None);
    }

    #[test]
    fn to_release_prefers_java_version_over_distribution_version() {
        // 测试后半段会改写字段，需要 mut；用字面量构造以通过 field_reassign lint
        let mut fj = FjPackage {
            distribution: "temurin".into(),
            java_version: "17.0.20.1+1".into(),
            distribution_version: "17.0.20.1".into(),
            major_version: 17,
            term_of_support: "lts".into(),
            ..FjPackage::default()
        };
        let r = FoojaySource::to_release(&fj).unwrap();
        assert_eq!(r.version, "17.0.20.1+1");
        assert_eq!(r.feature, 17);
        assert!(r.lts);
        assert_eq!(r.term, "LTS");

        // java_version 缺失时回退 distribution_version
        fj.java_version = String::new();
        let r = FoojaySource::to_release(&fj).unwrap();
        assert_eq!(r.version, "17.0.20.1");

        // 两个版本字段都为空 → 不可发布
        fj.distribution_version = String::new();
        assert!(FoojaySource::to_release(&fj).is_none());
    }
}
