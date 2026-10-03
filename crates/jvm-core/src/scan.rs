//! 本地 JDK 发现：只加载应用安装目录（`versions/`）下的 JDK。
//!
//! 此前会扫描 JAVA_HOME / Windows 注册表 / 常见系统位置 / 用户配置的
//! 额外目录里的"外部"JDK，实际暴露出一系列误报问题：`current` 链接与
//! JDK8 内嵌的 `jre/` 子目录被当成独立 JDK（路径经 junction 别名后连
//! 嵌套剪枝都失效），厂商还会从 `java -version` 输出解析出 `(build`
//! 之类的垃圾值。已按产品决策移除外部扫描：本地列表只加载 `versions/`
//! 下的目录；站外 JDK 请用「导入」拷入安装目录统一管理。
//!
//! 探测规则：存在 `bin/java(.exe)` 才算 JDK；版本/厂商优先读 `release`
//! 文件（发行商写入的 JAVA_VERSION / IMPLEMENTOR），缺失时回退执行
//! `java -version`（其输出不含可靠厂商字段，厂商宁可未知也不给错值）；
//! 剔除嵌套条目（如 JDK8 内嵌的 jre 子目录）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::home::Home;
use crate::installer::exe_name;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedJdk {
    /// JDK 主目录（bin/java 所在）。
    pub dir: String,
    /// 目录名（GUI「名称」列）。
    pub name: String,
    pub java_version: Option<String>,
    pub vendor: Option<String>,
    /// 是否位于应用安装目录内（扫描仅覆盖安装目录，恒为 true；
    /// 索引中的外部 JDK 记录仍依赖该字段语义，故保留）。
    pub in_install_dir: bool,
}

fn java_exe(dir: &Path) -> PathBuf {
    dir.join("bin").join(exe_name("java"))
}

/// JDK 目录探测结果。
#[derive(Debug, Clone, Default)]
pub struct JdkProbe {
    pub java_version: Option<String>,
    pub vendor: Option<String>,
}

/// 从 `release` 文件解析版本/厂商；没有该文件时执行
/// `java -version` 兜底（只能拿到版本号）。
pub fn probe_dir(dir: &Path) -> Option<JdkProbe> {
    if !dir.is_dir() || !java_exe(dir).is_file() {
        return None;
    }
    let release_file = dir.join("release");
    if let Ok(text) = std::fs::read_to_string(&release_file) {
        let mut probe = JdkProbe::default();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("JAVA_VERSION=") {
                probe.java_version = Some(v.trim_matches('"').to_string());
            } else if let Some(v) = line.strip_prefix("IMPLEMENTOR=") {
                probe.vendor = Some(v.trim_matches('"').to_string());
            }
        }
        if probe.java_version.is_some() || probe.vendor.is_some() {
            return Some(probe);
        }
    }
    // release 文件缺失/无字段：执行 java -version（输出在 stderr）取版本；
    // 输出里的 "Runtime Environment (build …)" 不是厂商名，不再解析。
    // 复用带 30s 超时的验证器（installer::run_java_version）：损坏的
    // 二进制可能挂死（Windows 缺 DLL 还会弹系统错误对话框），无超时等待
    // 会把调用方（GUI 本地列表加载）永久卡住。
    let text = crate::installer::run_java_version(dir).ok()?;
    let java_version = text
        .split("version \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string);
    Some(JdkProbe {
        java_version,
        ..JdkProbe::default()
    })
}

/// 扫描安装目录 `versions/`，返回去重、剪枝后的 JDK 列表（按版本降序）。
pub fn scan_local(home: &Home) -> Vec<ScannedJdk> {
    let install_root = home.versions();
    let mut out: Vec<ScannedJdk> = Vec::new();

    let Ok(entries) = std::fs::read_dir(&install_root) else {
        tracing::info!(install_root = %install_root.display(), "安装目录不可读，本地 JDK 列表为空");
        return out;
    };

    // 以规范化路径去重（Windows 不区分大小写）；解压残留的临时目录
    // （.tmp-*）没有 bin/java，会被 probe 自然过滤。
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let canonical = std::fs::canonicalize(&dir)
            .map(|p| crate::store::path_key(&p))
            .unwrap_or_else(|_| crate::store::path_key(&dir));
        if !seen.insert(canonical) {
            continue;
        }
        if let Some(probe) = probe_dir(&dir) {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.to_string_lossy().into_owned());
            out.push(ScannedJdk {
                dir: dir.to_string_lossy().into_owned(),
                name,
                java_version: probe.java_version,
                vendor: probe.vendor,
                // 扫描只覆盖安装目录，恒为 true。
                in_install_dir: true,
            });
        }
    }

    // 剔除嵌套在其它 JDK 内部的条目（如 JDK8 内嵌的 jre 子目录）。
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let is_descendant = |child: &str, ancestor: &str| {
        child != ancestor
            && child.starts_with(ancestor)
            && child.as_bytes().get(ancestor.len()) == Some(&(sep as u8))
    };
    let keys: Vec<String> = out
        .iter()
        .map(|j| crate::store::path_key(Path::new(&j.dir)))
        .collect();
    out.retain(|jdk| {
        let child = crate::store::path_key(Path::new(&jdk.dir));
        !keys.iter().any(|anc| is_descendant(&child, anc))
    });

    out.sort_by(|a, b| {
        b.java_version
            .as_deref()
            .unwrap_or("")
            .cmp(a.java_version.as_deref().unwrap_or(""))
    });
    tracing::info!(total = out.len(), install_root = %install_root.display(), "安装目录 JDK 扫描完成");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_requires_java_binary() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        // 无 bin/java（或 java.exe）→ 不算 JDK
        assert!(probe_dir(dir.path()).is_none());
        // release 文件可读时给出版本（Windows 上 java.exe 不存在同样不算，需二进制）
        #[cfg(unix)]
        {
            std::fs::write(dir.path().join("bin/java"), b"#!/bin/sh\n").unwrap();
            let probe = probe_dir(dir.path()).unwrap();
            assert!(probe.java_version.is_none(), "无 release 文件且无法执行时版本为空");
        }
    }

    #[test]
    fn probe_parses_release_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin").join(exe_name("java")), b"x").unwrap();
        std::fs::write(
            dir.path().join("release"),
            // JAVA_VERSION_DATE 等未采集字段应被忽略而不是报错
            "JAVA_VERSION=\"17.0.20.1\"\nIMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION_DATE=\"2024-07-16\"\n",
        )
        .unwrap();
        let probe = probe_dir(dir.path()).unwrap();
        assert_eq!(probe.java_version.as_deref(), Some("17.0.20.1"));
        assert_eq!(probe.vendor.as_deref(), Some("Eclipse Adoptium"));
    }

    #[test]
    fn release_file_parsing() {
        let dir = tempfile::tempdir().unwrap();
        // 仅验证 release 文件解析逻辑本身：直接构造并读取
        std::fs::write(
            dir.path().join("release"),
            "JAVA_VERSION=\"17.0.9\"\nIMPLEMENTOR=\"Oracle Corporation\"\n",
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("release")).unwrap();
        let get = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(k).map(|v| v.trim_matches('"').to_string()))
        };
        assert_eq!(get("JAVA_VERSION=").as_deref(), Some("17.0.9"));
        assert_eq!(get("IMPLEMENTOR=").as_deref(), Some("Oracle Corporation"));
    }

    #[test]
    fn scan_lists_versions_dir_only() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::from_root(tmp.path().join(".jvm"));
        home.ensure_dirs().unwrap();

        // JDK8：内嵌 jre 子目录（bin/java 齐全），扫描后应只剩外层
        let jdk8 = home.versions().join("oracle_open_jdk-8.0.342_7");
        std::fs::create_dir_all(jdk8.join("bin")).unwrap();
        std::fs::create_dir_all(jdk8.join("jre").join("bin")).unwrap();
        std::fs::write(jdk8.join("bin").join(exe_name("java")), b"x").unwrap();
        std::fs::write(jdk8.join("jre").join("bin").join(exe_name("java")), b"x").unwrap();
        std::fs::write(jdk8.join("release"), "JAVA_VERSION=\"1.8.0_342\"\n").unwrap();
        std::fs::write(jdk8.join("jre").join("release"), "JAVA_VERSION=\"1.8.0_342\"\n").unwrap();

        let jdk17 = home.versions().join("temurin_17.0.20.1_1");
        std::fs::create_dir_all(jdk17.join("bin")).unwrap();
        std::fs::write(jdk17.join("bin").join(exe_name("java")), b"x").unwrap();
        std::fs::write(
            jdk17.join("release"),
            "JAVA_VERSION=\"17.0.20.1\"\nIMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION_DATE=\"2024-07-16\"\n",
        )
        .unwrap();

        // 非 JDK 目录/文件被跳过
        std::fs::create_dir_all(home.versions().join("not-a-jdk")).unwrap();
        std::fs::write(home.versions().join("readme.txt"), b"x").unwrap();

        // 安装目录之外的 JDK 即使存在也不再进入结果（外部扫描已移除）
        let outside = tmp.path().join("outside-jdk");
        std::fs::create_dir_all(outside.join("bin")).unwrap();
        std::fs::write(outside.join("bin").join(exe_name("java")), b"x").unwrap();
        std::fs::write(outside.join("release"), "JAVA_VERSION=\"21\"\n").unwrap();

        let found = scan_local(&home);
        let names: Vec<&str> = found.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["temurin_17.0.20.1_1", "oracle_open_jdk-8.0.342_7"],
            "只加载 versions/ 下的 JDK，内嵌 jre 被剔除，按版本降序"
        );
        assert!(found.iter().all(|j| j.in_install_dir));
        let rec = found.iter().find(|j| j.name == "temurin_17.0.20.1_1").unwrap();
        assert_eq!(rec.vendor.as_deref(), Some("Eclipse Adoptium"));
        assert_eq!(rec.java_version.as_deref(), Some("17.0.20.1"));
    }
}
