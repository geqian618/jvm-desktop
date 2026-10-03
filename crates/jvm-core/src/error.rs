//! 统一错误类型。

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("网络请求失败: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("版本标识 `{0}` 不合法，应为 <vendor>@<version>，如 temurin@17.0.12+7")]
    InvalidId(String),

    #[error("未找到版本 `{0}`，可先执行 jvm ls-remote 查看")]
    NotFound(String),

    #[error("版本 `{0}` 已安装")]
    AlreadyInstalled(String),

    #[error("版本 `{0}` 正在使用中：它是全局默认版本或已被项目关联，请先切换/解除后再卸载")]
    InUse(String),

    #[error("校验和不匹配: 期望 {expected}, 实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("下载源未提供校验和且未找到 .sha256.txt，按安全策略拒绝安装")]
    NoChecksum,

    #[error("归档内未找到预期的 JDK 目录结构（bin/java 或 Contents/Home/bin/java）")]
    BadArchive,

    #[error("索引与磁盘不一致：{0}（可尝试 jvm doctor）")]
    IndexOutOfSync(String),

    #[error("验证 `java -version` 失败: {0}")]
    VerifyFailed(String),

    #[error("任务已取消")]
    Cancelled,

    #[error("{0}")]
    Message(String),
}

impl Error {
    pub fn msg(m: impl Into<String>) -> Self {
        Error::Message(m.into())
    }

    pub fn index_out_of_sync(p: impl Into<PathBuf>) -> Self {
        Error::IndexOutOfSync(p.into().display().to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
