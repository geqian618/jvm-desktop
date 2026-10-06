//! 核心领域模型：JDK 标识、版本引用、远程发行包、本机探测。

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// 全局唯一的 JDK 标识：`<vendor>@<version>`，如 `temurin@17.0.12+7`。
///
/// 同一 version 可以来自多个发行商，vendor 参与唯一性；磁盘目录名由
/// [`JdkId::to_dir_name`] 生成，与标识串保持一致。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JdkId {
    pub vendor: String,
    pub version: String,
}

impl JdkId {
    pub fn new(vendor: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            vendor: vendor.into(),
            version: version.into(),
        }
    }

    /// 解析 `vendor@version` 形式的字符串。
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        let (vendor, version) = s
            .split_once('@')
            .ok_or_else(|| Error::InvalidId(s.to_string()))?;
        if vendor.is_empty() || version.is_empty() {
            return Err(Error::InvalidId(s.to_string()));
        }
        let vendor = vendor.to_ascii_lowercase();
        if !vendor
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Error::InvalidId(s.to_string()));
        }
        Ok(Self {
            vendor,
            version: version.to_string(),
        })
    }

    /// `versions/` 下的目录名与下载归档文件名基底：`vendor@version` 原样
    /// 保留（`@`、`+` 在 Windows/macOS/Linux 文件名中都合法），仅兜底替换
    /// 个别非法字符，保证磁盘命名与列表展示的标识串一致。
    pub fn to_dir_name(&self) -> String {
        let sanitize = |s: &str| -> String {
            s.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '+') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        };
        format!("{}@{}", sanitize(&self.vendor), sanitize(&self.version))
    }
}

impl fmt::Display for JdkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.vendor, self.version)
    }
}

/// 用户输入的“宽松”版本引用：`17` / `17.0.12` / `17.0.12+7`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionRef {
    /// 仅主版本号，如 `17`：匹配该 feature 的最新版本。
    Feature(u32),
    /// 前缀匹配，如 `17.0.12` 匹配 `17.0.12+7`。
    Prefix(String),
    /// 完整版本串。
    Exact(String),
}

impl VersionRef {
    /// 解析用户输入；`vendor@` 前缀由调用方先行剥离。
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
            return VersionRef::Feature(s.parse().expect("digits checked"));
        }
        if s.contains('+') {
            VersionRef::Exact(s.to_string())
        } else {
            VersionRef::Prefix(s.to_string())
        }
    }

    /// 判断完整版本串是否命中该引用。
    pub fn matches(&self, version: &str) -> bool {
        match self {
            VersionRef::Exact(e) => version.eq_ignore_ascii_case(e),
            VersionRef::Prefix(p) => {
                version.starts_with(p)
                    && (version.len() == p.len()
                        || matches!(version.as_bytes()[p.len()], b'+' | b'.'))
            }
            VersionRef::Feature(n) => version_feature(version) == Some(*n),
        }
    }
}

/// 从版本串提取主版本号：`17.0.12+7` → 17，`25-ea+9` → 25。
pub fn version_feature(version: &str) -> Option<u32> {
    let head: String = version.chars().take_while(|c| c.is_ascii_digit()).collect();
    head.parse().ok()
}

/// 把版本串拆成可比较的数值组件与 build 号：`17.0.20.1+2` → ([17,0,20,1], 2)。
fn version_components(v: &str) -> (Vec<u64>, u64) {
    let (core, build) = match v.split_once('+') {
        Some((c, b)) => (c, b.parse::<u64>().unwrap_or(0)),
        None => (v, 0),
    };
    let comps = core
        .split('.')
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect();
    (comps, build)
}

/// 版本排序：逐段数值比较（缺失视为 0），再比较 build 号。
pub fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (ca, ba) = version_components(a);
    let (cb, bb) = version_components(b);
    let n = ca.len().max(cb.len());
    for i in 0..n {
        let x = ca.get(i).copied().unwrap_or(0);
        let y = cb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    ba.cmp(&bb)
}

/// 远程发行包中已按本机 os/arch/包类型选定的下载项。
#[derive(Debug, Clone)]
pub struct RemotePackage {
    pub os: String,
    pub arch: String,
    /// `zip`（Windows）或 `tar.gz`（macOS/Linux）。
    pub archive_type: String,
    pub size: Option<u64>,
    /// 归档文件名（GUI「文件」列展示）。
    pub filename: String,
    /// 发行商自己的版本号（GUI「发行版版本」列）。
    pub distribution_version: String,
    /// 包类型：jdk / jre（GUI「类型」列）。
    pub image_type: String,
    /// 直接下载链接；foojay 的列表响应不含直链，需经 `info_uri` 二次获取。
    pub direct_url: Option<String>,
    /// 包详情地址（foojay: `/ids/{id}`），用于补全直链与校验和。
    pub info_uri: Option<String>,
    pub checksum_sha256: Option<String>,
    pub checksum_url: Option<String>,
}

/// 一个可选安装的发行版本（已绑定当前平台的包）。
#[derive(Debug, Clone)]
pub struct Release {
    pub vendor: String,
    /// 完整版本串（含 `+build`），如 `17.0.12+7`。
    pub version: String,
    pub feature: u32,
    /// 支持类型：LTS / MTS / STS（GUI 筛选列）。
    pub term: String,
    pub lts: bool,
    pub package: RemotePackage,
}

impl Release {
    pub fn id(&self) -> JdkId {
        JdkId::new(self.vendor.clone(), self.version.clone())
    }
}

/// 本机操作系统；`std::env::consts::OS` 的取值（windows/macos/linux）
/// 与 foojay 的取值域一致，无需映射。
pub fn current_os() -> &'static str {
    std::env::consts::OS
}

/// 本机 CPU 架构，取 foojay 的取值域。
pub fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => other,
    }
}

/// 平台对应的归档类型：Windows 用 zip，其余 tar.gz。
pub fn platform_archive_type() -> &'static str {
    if cfg!(windows) {
        "zip"
    } else {
        "tar.gz"
    }
}

/// 下载文件名推测（源未给 filename 时的兜底）。
pub fn archive_filename(id: &JdkId, archive_type: &str) -> String {
    let ext = if archive_type == "zip" {
        "zip"
    } else {
        "tar.gz"
    };
    format!("{}.{}", id.to_dir_name(), ext)
}

/// serde 形式：整个 `JdkId` 序列化为单字符串 `vendor@version`。
impl Serialize for JdkId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for JdkId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        JdkId::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let id = JdkId::parse("Temurin@17.0.12+7").unwrap();
        assert_eq!(id.vendor, "temurin");
        assert_eq!(id.version, "17.0.12+7");
        assert_eq!(id.to_string(), "temurin@17.0.12+7");
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(JdkId::parse("temurin17").is_err());
        assert!(JdkId::parse("@17.0.12").is_err());
        assert!(JdkId::parse("temurin@").is_err());
        assert!(JdkId::parse("te murin@17").is_err());
    }

    #[test]
    fn dir_name_keeps_safe_chars() {
        // `@` 与 `+` 原样保留，磁盘目录名与标识串一致
        assert_eq!(
            JdkId::parse("temurin@17.0.12+7").unwrap().to_dir_name(),
            "temurin@17.0.12+7"
        );
        assert_eq!(
            JdkId::parse("zulu@25.0.1+8").unwrap().to_dir_name(),
            "zulu@25.0.1+8"
        );
        assert_eq!(
            JdkId::new("graalvm_community", "25-ea+9").to_dir_name(),
            "graalvm_community@25-ea+9"
        );
    }

    #[test]
    fn version_ref_matching() {
        assert!(VersionRef::parse("17").matches("17.0.12+7"));
        assert!(VersionRef::parse("17").matches("17+35"));
        assert!(!VersionRef::parse("17").matches("25.0.1+8"));
        assert!(VersionRef::parse("17.0.12").matches("17.0.12+7"));
        assert!(!VersionRef::parse("17.0.1").matches("17.0.12+7"));
        assert!(VersionRef::parse("17.0.12+7").matches("17.0.12+7"));
        assert!(!VersionRef::parse("17.0.12+7").matches("17.0.12+8"));
    }

    #[test]
    fn version_ordering() {
        assert_eq!(
            cmp_versions("17.0.12+7", "17.0.12+8"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            cmp_versions("17.0.20.1+1", "17.0.12+7"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            cmp_versions("17.0.12+7", "17.0.12"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            cmp_versions("8.0.1+3", "8.0.1+3"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn feature_extraction() {
        assert_eq!(version_feature("17.0.12+7"), Some(17));
        assert_eq!(version_feature("17+35"), Some(17));
        assert_eq!(version_feature("25.0.4.1+62369"), Some(25));
        assert_eq!(version_feature("ea"), None);
    }

    #[test]
    fn serde_roundtrip() {
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"temurin@17.0.12+7\"", "整个 JdkId 序列化为单字符串");
        let back: JdkId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(serde_json::from_str::<JdkId>("\"bad-format\"").is_err());
    }

    #[test]
    fn version_ref_exact_is_case_insensitive() {
        assert!(VersionRef::parse("17.0.12+7").matches("17.0.12+7"));
        let upper: String = "17.0.12+7".to_uppercase();
        assert!(VersionRef::parse("17.0.12+7").matches(&upper));
    }
}
