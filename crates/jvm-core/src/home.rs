//! 数据目录布局与路径计算。
//!
//! 根目录优先取 `JVM_HOME` 环境变量（测试与多实例场景），
//! 否则为用户主目录下的 `.jvm`。
//!
//! 布局：
//! ```text
//! ~/.jvm/
//! ├── versions/<vendor>_<version>/   已安装的 JDK
//! ├── bin/                           java 等高频命令的 shim 与 jvm 自身
//! ├── current                        指向当前默认版本的链接（Windows 为 junction）
//! ├── downloads/                     下载缓存
//! ├── logs/                          滚动日志与环境变量备份
//! ├── default                        单行 vendor@version
//! ├── index.json                     本地已装版本索引
//! ├── projects.json                  项目 → 版本绑定
//! └── setting.json                   数据源/代理/语言/主题
//! ```

use std::path::{Path, PathBuf};

use crate::model::JdkId;

#[derive(Debug, Clone)]
pub struct Home {
    root: PathBuf,
    versions_override: Option<PathBuf>,
    downloads_override: Option<PathBuf>,
}

impl Home {
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            versions_override: None,
            downloads_override: None,
        }
    }

    /// 按设置应用安装目录/下载路径覆盖（设置里留空则用默认位置）。
    pub fn with_settings(mut self, settings: &crate::store::Settings) -> Self {
        let s = |v: &str| !v.trim().is_empty();
        if s(&settings.install_dir) {
            self.versions_override = Some(PathBuf::from(settings.install_dir.trim()));
        }
        if s(&settings.download_dir) {
            self.downloads_override = Some(PathBuf::from(settings.download_dir.trim()));
        }
        self
    }

    /// detect + 读取设置并应用目录覆盖；GUI 与 CLI 的统一入口。
    pub fn configured() -> Self {
        let home = Self::detect();
        match crate::store::load_settings(&home) {
            Ok(settings) => home.with_settings(&settings),
            Err(_) => home,
        }
    }

    pub fn detect() -> Self {
        let root = match std::env::var_os("JVM_HOME").filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            // 旧版自定义位置：尊重用户既有配置，不做迁移
            None => match std::env::var_os("JDM_HOME").filter(|v| !v.is_empty()) {
                Some(v) => PathBuf::from(v),
                None => {
                    let default_root = user_home().join(".jvm");
                    crate::migrate::migrate_legacy_default_root(&default_root);
                    default_root
                }
            },
        };
        Self {
            root,
            versions_override: None,
            downloads_override: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn versions(&self) -> PathBuf {
        self.versions_override
            .clone()
            .unwrap_or_else(|| self.root.join("versions"))
    }

    /// 某 JDK 的安装目录（不保证存在）。
    pub fn version_dir(&self, id: &JdkId) -> PathBuf {
        self.versions().join(id.to_dir_name())
    }

    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn downloads(&self) -> PathBuf {
        self.downloads_override
            .clone()
            .unwrap_or_else(|| self.root.join("downloads"))
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `current` 链接路径（JAVA_HOME 指向这里，切换只重建链接）。
    pub fn current(&self) -> PathBuf {
        self.root.join("current")
    }

    pub fn default_file(&self) -> PathBuf {
        self.root.join("default")
    }

    pub fn index_file(&self) -> PathBuf {
        self.root.join("index.json")
    }

    pub fn projects_file(&self) -> PathBuf {
        self.root.join("projects.json")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("setting.json")
    }

    /// 创建全部子目录；已存在时静默通过。
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [self.versions(), self.bin(), self.downloads(), self.logs()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

fn user_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Settings;

    #[test]
    fn paths_are_consistent() {
        let home = Home::from_root("/tmp/jvm-test");
        assert_eq!(home.versions(), PathBuf::from("/tmp/jvm-test/versions"));
        assert_eq!(home.current(), PathBuf::from("/tmp/jvm-test/current"));

        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        assert_eq!(
            home.version_dir(&id),
            PathBuf::from("/tmp/jvm-test/versions/temurin_17.0.12_7")
        );
    }

    #[test]
    fn with_settings_overrides_install_and_download_dirs() {
        let settings = Settings {
            install_dir: "D:/jdks".into(),
            download_dir: "E:/dl".into(),
            ..Settings::default()
        };
        let home = Home::from_root("/r").with_settings(&settings);
        assert_eq!(home.versions(), PathBuf::from("D:/jdks"));
        assert_eq!(home.downloads(), PathBuf::from("E:/dl"));
        // 其余路径仍基于 root
        assert_eq!(home.bin(), PathBuf::from("/r/bin"));
        assert_eq!(home.logs(), PathBuf::from("/r/logs"));

        // 留空的设置项不覆盖默认位置
        let home2 = Home::from_root("/r").with_settings(&Settings::default());
        assert_eq!(home2.versions(), PathBuf::from("/r/versions"));
        assert_eq!(home2.downloads(), PathBuf::from("/r/downloads"));
    }
}
