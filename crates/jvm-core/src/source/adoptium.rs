//! Adoptium API v3 客户端（Temurin 专用加速源）。
//!
//! Adoptium 只提供 Eclipse Temurin（vendor 固定为 `temurin`）；
//! 相比 foojay 的优势是列表响应直接携带直链与校验和，少一次请求。

use std::sync::Arc;

use futures_util::StreamExt;
use serde::Deserialize;

use super::{DistributionInfo, MajorVersion, PackageDetail, PkgQuery, RegistrySource};
use crate::error::Result;
use crate::model::{Release, RemotePackage};

#[derive(Debug, Clone)]
pub struct AdoptiumSource {
    base: String,
    client: reqwest::Client,
}
#[derive(Deserialize)]
struct AsReleases {
    #[serde(default)]
    available_releases: Vec<u32>,
    #[serde(default)]
    available_lts_releases: Vec<u32>,
}

#[derive(Deserialize)]
struct AsAsset {
    #[serde(rename = "release_name")]
    release_name: String,
    /// 同一 API 两种响应形状：`/assets/feature_releases/*` 返回 `binaries`
    /// 数组，`/assets/latest/*` 返回 `binary` 单对象——两者都要兼容。
    /// 每个 binary 里 package 是压缩包，installer 是 msi 等安装器（可能缺省）。
    #[serde(default)]
    binaries: Vec<AsBinary>,
    #[serde(default)]
    binary: Option<AsBinary>,
}

impl AsAsset {
    /// 归一两种形状，产出其中携带的所有压缩包。
    fn packages(&self) -> impl Iterator<Item = &AsPackage> {
        self.binaries
            .iter()
            .filter_map(|b| b.package.as_ref())
            .chain(self.binary.iter().filter_map(|b| b.package.as_ref()))
    }
}

#[derive(Deserialize)]
struct AsBinary {
    #[serde(default)]
    package: Option<AsPackage>,
}

#[derive(Deserialize)]
struct AsPackage {
    #[serde(default)]
    name: String,
    link: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    checksum: Option<String>,
}

/// URL 匹配器（SPECS 注册表用）：只认 Adoptium 官方地址。
pub fn try_create(url: &str, _api_key: &str, client: reqwest::Client) -> Option<super::Source> {
    if url != super::ADOPTIUM_BASE {
        return None;
    }
    Some(Arc::new(AdoptiumSource::new(client)))
}

impl AdoptiumSource {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            base: super::ADOPTIUM_BASE.to_string(),
            client,
        }
    }

    /// Adoptium 的 os 取值与 foojay 不同：macOS 是 `mac` 而非 `macos`。
    fn map_os(os: &str) -> String {
        match os {
            "macos" => "mac".to_string(),
            other => other.to_string(),
        }
    }

    fn release_version(release_name: &str) -> Option<&str> {
        release_name.strip_prefix("jdk-").filter(|s| !s.is_empty())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let resp = self
            .client
            .get(format!("{}{path}", self.base))
            .query(query)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json::<T>().await?)
    }

    fn asset_to_release(
        asset: &AsAsset,
        feature: u32,
        lts: bool,
        os: &str,
        arch: &str,
        image_type: &str,
    ) -> Option<Release> {
        let version = Self::release_version(&asset.release_name)?.to_string();
        let pkg = asset.packages().next()?;
        Some(Release {
            vendor: "temurin".to_string(),
            version: version.clone(),
            feature,
            term: if lts {
                "LTS".to_string()
            } else {
                "STS".to_string()
            },
            lts,
            package: RemotePackage {
                os: os.to_string(),
                arch: arch.to_string(),
                archive_type: crate::model::platform_archive_type().to_string(),
                size: (pkg.size > 0).then_some(pkg.size),
                filename: pkg.name.clone(),
                distribution_version: version.clone(),
                image_type: image_type.to_string(),
                direct_url: Some(pkg.link.clone()),
                info_uri: None,
                checksum_sha256: pkg
                    .checksum
                    .as_ref()
                    .map(|c| c.trim().to_lowercase())
                    .filter(|c| !c.is_empty()),
                checksum_url: None,
            },
        })
    }

    async fn assets(&self, query: &PkgQuery, latest_only: bool) -> Result<Vec<AsAsset>> {
        let os = Self::map_os(&query.os);
        let path = if latest_only {
            format!("/assets/latest/{}/hotspot", query.feature)
        } else {
            format!("/assets/feature_releases/{}/ga", query.feature)
        };
        let params = [
            ("os", os.as_str()),
            ("architecture", query.arch.as_str()),
            ("image_type", query.image_type.as_str()),
        ];
        let resp = self
            .client
            .get(format!("{}{path}", self.base))
            .query(&params)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json::<Vec<AsAsset>>().await?)
    }
}

/// [`RegistrySource`] 实现：Adoptium 列表自带直链与校验和，无需二次查询。
/// 私有辅助（map_os/get_json/asset_to_release/assets/is_lts）留在固有 impl。
#[async_trait::async_trait]
impl RegistrySource for AdoptiumSource {
    fn spec_name(&self) -> &'static str {
        "adoptium"
    }

    async fn latest_for_feature(&self, query: &PkgQuery) -> Result<Option<Release>> {
        let lts = self.is_lts(query.feature).await;
        let assets = self.assets(query, true).await?;
        Ok(assets.first().and_then(|a| {
            Self::asset_to_release(
                a,
                query.feature,
                lts,
                &query.os,
                &query.arch,
                &query.image_type,
            )
        }))
    }

    async fn package_for_exact(&self, query: &PkgQuery, version: &str) -> Result<Option<Release>> {
        let lts = self.is_lts(query.feature).await;
        let want = format!("jdk-{version}");
        let assets = self.assets(query, false).await?;
        Ok(assets
            .iter()
            .find(|a| a.release_name == want)
            .and_then(|a| {
                Self::asset_to_release(
                    a,
                    query.feature,
                    lts,
                    &query.os,
                    &query.arch,
                    &query.image_type,
                )
            }))
    }

    async fn distributions(&self) -> Result<Vec<DistributionInfo>> {
        Ok(vec![DistributionInfo {
            api_name: "temurin".to_string(),
            name: "Eclipse Temurin".to_string(),
            maintained: true,
            versions: Vec::new(),
        }])
    }

    async fn major_versions(&self) -> Result<Vec<MajorVersion>> {
        let info: AsReleases = self.get_json("/info/available_releases", &[]).await?;
        let mut feats: Vec<MajorVersion> = info
            .available_releases
            .iter()
            .map(|f| MajorVersion {
                major_version: *f,
                term_of_support: if info.available_lts_releases.contains(f) {
                    "LTS".to_string()
                } else {
                    "STS".to_string()
                },
                maintained: true,
                versions: Vec::new(),
            })
            .collect();
        feats.sort_by_key(|m| m.major_version);
        Ok(feats)
    }

    /// 全部 GA 包：对每个主版本调一次 feature_releases（并发 4），
    /// 客户端按归档扩展名过滤。
    async fn all_packages(
        &self,
        _vendor: &str,
        os: &str,
        arch: &str,
        archive_type: &str,
    ) -> Result<Vec<Release>> {
        let majors = self.major_versions().await?;
        let lts_set: std::collections::HashSet<u32> = majors
            .iter()
            .filter(|m| m.term_of_support == "LTS")
            .map(|m| m.major_version)
            .collect();

        let suffix = if archive_type == "zip" {
            ".zip"
        } else {
            ".tar.gz"
        };
        let queries: Vec<PkgQuery> = majors
            .iter()
            .map(|m| PkgQuery {
                vendor: "temurin".to_string(),
                feature: m.major_version,
                os: os.to_string(),
                arch: arch.to_string(),
                archive_type: archive_type.to_string(),
                image_type: "jdk".to_string(),
            })
            .collect();

        let results = futures_util::stream::iter(queries)
            .map(|q| {
                let this = self.clone();
                let lts = lts_set.clone();
                async move {
                    // 单个主版本失败只记日志跳过（其余主版本照常返回），
                    // 但错误要向上传递，供全失败时整体报错。
                    match this.assets(&q, false).await {
                        Ok(a) => Ok(a
                            .iter()
                            .filter(|a| a.packages().any(|p| p.name.ends_with(suffix)))
                            .filter_map(|a| {
                                Self::asset_to_release(
                                    a,
                                    q.feature,
                                    lts.contains(&q.feature),
                                    &q.os,
                                    &q.arch,
                                    &q.image_type,
                                )
                            })
                            .collect::<Vec<_>>()),
                        Err(e) => {
                            tracing::warn!(feature = q.feature, error = %e, "Adoptium 拉取主版本包列表失败");
                            Err(e)
                        }
                    }
                }
            })
            .buffered(4)
            .collect::<Vec<crate::error::Result<Vec<Release>>>>()
            .await;

        let mut out: Vec<Release> = Vec::new();
        let mut failed = 0usize;
        for r in results {
            match r {
                Ok(list) => out.extend(list),
                Err(_) => failed += 1,
            }
        }
        // 全部主版本都失败（如 os 参数无效、网络/代理故障）必须向上报错，
        // 否则调用方只看到空列表而无任何提示，问题无从排查。
        if out.is_empty() && failed > 0 {
            return Err(crate::error::Error::msg(format!(
                "Adoptium 拉取包列表失败（{failed} 个主版本全部失败），请检查网络/代理或数据源设置"
            )));
        }
        out.sort_by(|a, b| crate::model::cmp_versions(&b.version, &a.version));
        Ok(out)
    }

    /// `feature_releases` 端点天然列出全部 GA 更新，比 foojay 准确。
    async fn version_strings(&self, _vendor: &str, feature: u32) -> Result<Vec<String>> {
        let query = PkgQuery::for_local_machine("temurin", feature);
        let assets = self.assets(&query, false).await?;
        let mut versions: Vec<String> = assets
            .iter()
            .filter_map(|a| Self::release_version(&a.release_name).map(str::to_string))
            .collect();
        versions.sort_by(|a, b| crate::model::cmp_versions(b, a));
        versions.dedup();
        Ok(versions)
    }

    async fn package_detail_by_uri(&self, _uri: &str) -> Result<PackageDetail> {
        // 列表响应已携带直链与校验和，无独立的包详情端点
        Ok(PackageDetail::default())
    }
}

impl AdoptiumSource {
    async fn is_lts(&self, feature: u32) -> bool {
        self.major_versions().await.is_ok_and(|majors| {
            majors
                .iter()
                .any(|m| m.major_version == feature && m.term_of_support == "LTS")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::version_feature;

    #[test]
    fn release_name_to_version() {
        assert_eq!(
            AdoptiumSource::release_version("jdk-17.0.12+7"),
            Some("17.0.12+7")
        );
        assert_eq!(AdoptiumSource::release_version("17.0.12+7"), None);
    }

    #[test]
    fn os_mapping() {
        assert_eq!(AdoptiumSource::map_os("macos"), "mac");
        assert_eq!(AdoptiumSource::map_os("windows"), "windows");
    }

    #[test]
    fn parses_asset_shape() {
        // 真实 API 形状：binaries 是数组，binary 里 package（压缩包）与
        // installer（msi 等）并存。回归测试：此前按 "binary" 单数对象解析，
        // 字段对不上导致每条响应反序列化失败、包列表静默为空。
        let body = r#"[{"release_name":"jdk-17.0.12+7","release_type":"ga","term_of_support":"LTS",
            "binaries":[{"architecture":"x64","heap_size":"normal","image_type":"jdk","os":"windows",
            "installer":{"name":"OpenJDK17U-jdk_x64_windows_hotspot_17.0.12_7.msi","link":"https://github.com/x/y.msi"},
            "package":{"name":"OpenJDK17U-jdk_x64_windows_hotspot_17.0.12_7.zip",
            "link":"https://github.com/x/y.zip","size":189575306,
            "checksum":"e53a79c3c3d86865bd7e787903884331068e71321714ffd44f145785affc7cb0"}}]}]"#;
        let assets: Vec<AsAsset> = serde_json::from_str(body).unwrap();
        let release =
            AdoptiumSource::asset_to_release(&assets[0], 17, true, "windows", "x64", "jdk")
                .unwrap();
        assert_eq!(release.version, "17.0.12+7");
        assert_eq!(release.vendor, "temurin");
        assert_eq!(
            release.package.filename,
            "OpenJDK17U-jdk_x64_windows_hotspot_17.0.12_7.zip"
        );
        assert!(release.package.direct_url.is_some());
        assert!(release.package.checksum_sha256.is_some());
        assert_eq!(release.package.size, Some(189575306));
    }

    #[test]
    fn parses_latest_singular_binary_shape() {
        // /assets/latest/* 返回 binary 单对象（非 binaries 数组）。
        // 回归测试：只认一种形状时，另一条路径的包会静默消失。
        let body = r#"[{"release_name":"jdk-17.0.20.1+1","release_type":"ga",
            "binary":{"architecture":"x64","image_type":"jdk","os":"windows",
            "package":{"name":"OpenJDK17U-jdk_x64_windows_hotspot_17.0.20.1_1.zip",
            "link":"https://github.com/x/y.zip","size":190000000,
            "checksum":"e53a79c3c3d86865bd7e787903884331068e71321714ffd44f145785affc7cb0"}}}]"#;
        let assets: Vec<AsAsset> = serde_json::from_str(body).unwrap();
        let release =
            AdoptiumSource::asset_to_release(&assets[0], 17, true, "windows", "x64", "jdk")
                .unwrap();
        assert_eq!(release.version, "17.0.20.1+1");
        assert_eq!(release.package.size, Some(190000000));
        assert!(release.package.direct_url.is_some());
    }

    #[test]
    fn feature_from_version_helper() {
        assert_eq!(version_feature("17.0.12+7"), Some(17));
    }
}
