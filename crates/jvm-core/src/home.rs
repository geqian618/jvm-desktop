//! 数据目录布局与路径计算。
//!
//! 根目录优先取 `JVM_HOME` 环境变量（测试与多实例场景），
//! 否则为安装路径（应用 exe 所在目录）下的 `workspace`。
//!
//! 布局：
//! ```text
//! <安装路径>/workspace/
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
            None => default_root(),
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

/// 默认数据根：安装路径（当前 exe 所在目录）下的 `workspace`。
///
/// 平台兼容说明：
/// - Windows：GUI 与 CLI 本体都在安装目录，直接取 exe 目录；
/// - CLI 常驻拷贝位于 `<数据根>/bin/jvm`（workspace 之内），此时 exe
///   目录的上一级就是数据根，避免把 workspace 递归套进 bin 里；
/// - Unix 同样按 exe 目录推导（macOS 下 exe 在 `*.app/Contents/MacOS`，
///   数据根会落在 bundle 内；用户可用 `JVM_HOME` 环境变量覆盖到任意
///   可写位置，shell 提示文案里给出的就是实际生效路径）；
/// - exe 路径不可得时退回用户主目录下的 `.jvm`。
fn default_root() -> PathBuf {
    match std::env::current_exe() {
        Ok(exe) => root_for_exe(&exe),
        Err(_) => user_home().join(".jvm"),
    }
}

/// 由 exe 路径推导默认数据根（[`default_root`] 的纯函数形态，便于测试）。
fn root_for_exe(exe: &Path) -> PathBuf {
    let Some(dir) = exe.parent() else {
        return user_home().join(".jvm");
    };
    let is_cli_in_bin = exe
        .file_stem()
        .map(|s| s.eq_ignore_ascii_case("jvm"))
        .unwrap_or(false)
        && dir.file_name().map(|n| n == "bin").unwrap_or(false);
    if is_cli_in_bin {
        dir.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dir.join("workspace"))
    } else {
        dir.join("workspace")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Settings;

    #[test]
    fn default_root_follows_exe_location() {
        // GUI / CLI 本体都在安装目录：数据根是其下的 workspace
        #[cfg(windows)]
        let exe = Path::new("C:\\apps\\JVM Desktop.exe");
        #[cfg(not(windows))]
        let exe = Path::new("/opt/jvmdesktop/jvm-desktop");
        assert_eq!(root_for_exe(exe), exe.parent().unwrap().join("workspace"));

        // CLI 常驻拷贝在 <数据根>/bin/jvm：数据根是 exe 目录的上一级
        #[cfg(windows)]
        let shim = Path::new("C:\\apps\\workspace\\bin\\jvm.exe");
        #[cfg(not(windows))]
        let shim = Path::new("/opt/jvmdesktop/workspace/bin/jvm");
        assert_eq!(root_for_exe(shim), shim.parent().unwrap().parent().unwrap());

        // 名为 bin 但 exe 不是 CLI 本体：按普通安装目录处理
        #[cfg(windows)]
        let other = Path::new("C:\\apps\\bin\\tool.exe");
        #[cfg(not(windows))]
        let other = Path::new("/opt/jvmdesktop/bin/tool");
        assert_eq!(
            root_for_exe(other),
            other.parent().unwrap().join("workspace")
        );
    }

    #[test]
    fn paths_are_consistent() {
        let home = Home::from_root("/tmp/jvm-test");
        assert_eq!(home.versions(), PathBuf::from("/tmp/jvm-test/versions"));
        assert_eq!(home.current(), PathBuf::from("/tmp/jvm-test/current"));

        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        assert_eq!(
            home.version_dir(&id),
            PathBuf::from("/tmp/jvm-test/versions/temurin@17.0.12+7")
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
