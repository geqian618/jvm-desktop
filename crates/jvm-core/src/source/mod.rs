//! 版本源抽象：按 API 规范拆分的解析策略（foojay、Adoptium、…）。
//!
//! 每种 API 规范是 [`source::RegistrySource`] 的一个自包含实现——HTTP 客户端、
//! 响应 DTO、解析逻辑都收在自己的模块里（foojay.rs / adoptium.rs / …），只把
//! 结果归一化为本模块的模型（[`Release`] / [`DistributionInfo`] /
//! [`MajorVersion`] / [`PackageDetail`]）。上层（flow / CLI / GUI）只面向
//! trait 编程，新增规范对它们零改动。
//!
//! 扩展步骤：在 source/ 下新建模块 → 实现 [`RegistrySource`]（含
//! [`RegistrySource::spec_name`]）→ 提供 URL 匹配器
//! `try_create(url, api_key, client) -> Option<Source>` → 加入 [`SPECS`]
//! 注册表（放在全匹配兜底之前）。调用 [`from_settings`] 即按注册表顺序
//! 选出首个命中的规范。

pub mod adoptium;
pub mod foojay;

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::model::Release;
use crate::store::Settings;

pub const FOOJAY_BASE: &str = "https://api.foojay.io/disco/v3.0";
pub const ADOPTIUM_BASE: &str = "https://api.adoptium.net/v3";

/// 发行版元信息（foojay /distributions）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionInfo {
    pub api_name: String,
    pub name: String,
    pub maintained: bool,
    pub versions: Vec<String>,
}

/// 主版本元信息（foojay /major_versions）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MajorVersion {
    pub major_version: u32,
    pub term_of_support: String,
    pub maintained: bool,
    pub versions: Vec<String>,
}

/// 包详情：直链 + 校验和。按 `pkg_info_uri` 直查（foojay `/ids/{id}`），
/// 绕过按版本串重新匹配的不可靠路径（部分包 version= 过滤命中不到）。
#[derive(Debug, Clone, Default)]
pub struct PackageDetail {
    pub direct_url: Option<String>,
    pub checksum_sha256: Option<String>,
    pub checksum_url: Option<String>,
}

/// 对某个发行商、某个主版本的查询（os/arch/归档类型按本机自动填充）。
#[derive(Debug, Clone)]
pub struct PkgQuery {
    pub vendor: String,
    pub feature: u32,
    pub os: String,
    pub arch: String,
    pub archive_type: String,
    /// `jdk` 或 `jre`。
    pub image_type: String,
}

impl PkgQuery {
    pub fn for_local_machine(vendor: &str, feature: u32) -> Self {
        Self {
            vendor: vendor.to_string(),
            feature,
            os: crate::model::current_os().to_string(),
            arch: crate::model::current_arch().to_string(),
            archive_type: crate::model::platform_archive_type().to_string(),
            image_type: "jdk".to_string(),
        }
    }

    pub fn with_image_type(mut self, image_type: &str) -> Self {
        self.image_type = image_type.to_string();
        self
    }
}

/// 数据源策略接口：一种 API 规范一个实现，实现之间互不耦合。
#[async_trait::async_trait]
pub trait RegistrySource: Send + Sync {
    /// 规范名（CLI 展示 / 日志 / 测试断言用）：如 "foojay"、"adoptium"。
    fn spec_name(&self) -> &'static str;

    /// 发行商元信息列表（「所有版本」页的发行版下拉）。
    async fn distributions(&self) -> Result<Vec<DistributionInfo>>;

    /// 主版本列表（升序，含 LTS 标记与维护状态）。
    async fn major_versions(&self) -> Result<Vec<MajorVersion>>;

    /// 某发行商某主版本的最新 GA 版本（含本机可用的包）。
    async fn latest_for_feature(&self, query: &PkgQuery) -> Result<Option<Release>>;

    /// 精确版本 → Release；同时补全下载直链与校验和。
    async fn package_for_exact(
        &self,
        query: &PkgQuery,
        version: &str,
    ) -> Result<Option<Release>>;

    /// 拉取一个发行商在指定平台/归档类型下的全部 GA 包（不带 version 参数，
    /// 规范自行决定单请求还是按主版本聚合；大版本/维护状态/支持类型筛选由
    /// 调用方在本地完成）。
    async fn all_packages(
        &self,
        vendor: &str,
        os: &str,
        arch: &str,
        archive_type: &str,
    ) -> Result<Vec<Release>>;

    /// 某发行商某主版本的全部 GA 版本串（降序，仅版本号，无包详情）。
    async fn version_strings(&self, vendor: &str, feature: u32) -> Result<Vec<String>>;

    /// 按包详情 URI（foojay `/ids/{id}`）补全直链与校验和；
    /// 列表已自带直链的规范返回空详情，调用方保持原值。
    async fn package_detail_by_uri(&self, uri: &str) -> Result<PackageDetail>;

    /// 全部维护中的主版本号，升序。
    async fn features(&self) -> Result<Vec<(u32, bool)>> {
        let majors = self.major_versions().await?;
        Ok(majors
            .iter()
            .filter(|m| m.maintained)
            .map(|m| (m.major_version, m.term_of_support.eq_ignore_ascii_case("lts")))
            .collect())
    }

    /// 全部维护中的 LTS 主版本号，升序。
    async fn lts_features(&self) -> Result<Vec<u32>> {
        Ok(self
            .features()
            .await?
            .into_iter()
            .filter(|(_, lts)| *lts)
            .map(|(f, _)| f)
            .collect())
    }
}

/// 任意 API 规范解析策略的通用句柄。
pub type Source = Arc<dyn RegistrySource>;

/// 规范工厂：URL + API Key + HTTP 客户端 → URL 命中该规范时返回其解析策略。
type SpecFactory = fn(&str, &str, reqwest::Client) -> Option<Source>;

/// 内置规范注册表：按序用 URL 匹配，首个命中者创建对应解析策略。
/// adoptium 只认官方地址；foojay 是全匹配兜底（任意 URL 都按 foojay
/// 兼容镜像对接），必须放最后。新增规范把自己的 try_create 插在兜底之前。
const SPECS: &[SpecFactory] = &[adoptium::try_create, foojay::try_create];

/// 按设置选择数据源策略。
///
/// registry 语义：空 / Foojay 官方 → foojay（官方地址）；Adoptium 官方 →
/// adoptium；其余 URL → foojay 兼容镜像（原样以该 URL 为 base 对接）。
pub fn from_settings(settings: &Settings) -> Result<Source> {
    let client = crate::net::build_client(settings)?;
    let url = settings.registry.trim().trim_end_matches('/');
    let api_key = settings.source_api_key.trim();
    for create in SPECS {
        if let Some(source) = create(url, api_key, client.clone()) {
            return Ok(source);
        }
    }
    // SPECS 末位是 foojay 全匹配兜底，到这里说明注册表被改坏了
    Err(Error::msg("SPECS 注册表缺少兜底匹配器"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Settings;

    fn settings_with(registry: &str) -> Settings {
        Settings {
            registry: registry.to_string(),
            ..Settings::default()
        }
    }

    #[test]
    fn from_settings_selects_spec_by_url() {
        // 空 registry → foojay 官方
        assert_eq!(
            from_settings(&settings_with("")).unwrap().spec_name(),
            "foojay"
        );
        // Foojay 官方地址 → foojay
        assert_eq!(
            from_settings(&settings_with(FOOJAY_BASE))
                .unwrap()
                .spec_name(),
            "foojay"
        );
        // Adoptium 官方地址（尾斜杠也应命中）→ adoptium
        assert_eq!(
            from_settings(&settings_with(&format!("{ADOPTIUM_BASE}/")))
                .unwrap()
                .spec_name(),
            "adoptium"
        );
        // 其余 URL → foojay 兼容镜像（仍是 foojay 规范）
        assert_eq!(
            from_settings(&settings_with("https://mirror.local/disco/v3.0"))
                .unwrap()
                .spec_name(),
            "foojay"
        );
    }
}