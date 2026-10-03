//! jvm-core：JDK 版本管理工具的核心库。
//!
//! 承载全部业务逻辑（版本源、下载、安装、切换、项目绑定），不含任何
//! Tauri/CLI 依赖；GUI（jvm-tauri）与命令行（jvm-cli）都是它的薄壳。
//!
//! 所有文件路径经由 [`crate::home::Home`] 注入（支持 `JVM_HOME` 环境变量
//! 覆盖），单元测试使用临时目录，不读写真实用户目录。

pub mod downloader;
pub mod env;
pub mod error;
pub mod flow;
pub mod home;
pub mod installer;
pub mod logging;
mod migrate;
pub mod model;
pub mod net;
pub mod project;
pub mod scan;
pub mod shim;
pub mod source;
pub mod store;
pub mod switcher;

pub use error::{Error, Result};
pub use home::Home;
pub use model::JdkId;
