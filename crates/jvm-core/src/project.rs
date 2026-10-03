//! 版本解析：`.jvmrc` > projects.json 最近项目根 > 全局 default。
//!
//! shim 与 CLI 共用同一套优先级，保证 `java -version` 与
//! `jvm current` 在任意目录下结论一致。

use std::path::{Path, PathBuf};

use crate::home::Home;
use crate::model::JdkId;
use crate::store;

/// 项目级绑定文件：单行 `vendor@version`，可提交到 Git 以保证团队一致。
pub const JVMRC_FILE: &str = ".jvmrc";

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// 命中 `.jvmrc`（项目根目录，绑定的 JDK）。
    RcFile { root: PathBuf, id: JdkId },
    /// 命中 `projects.json` 登记的项目根。
    Registry { root: PathBuf, id: JdkId },
    /// 回退到全局默认。
    Global(JdkId),
}

impl Resolution {
    pub fn id(&self) -> &JdkId {
        match self {
            Resolution::RcFile { id, .. } | Resolution::Registry { id, .. } => id,
            Resolution::Global(id) => id,
        }
    }
}

/// 从 `start` 目录开始解析生效的 JDK。
pub fn resolve(start: &Path, home: &Home) -> Option<Resolution> {
    // 1. 从 cwd 向上逐级查找 .jvmrc
    for dir in start.ancestors() {
        let candidate = dir.join(JVMRC_FILE);
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            if let Some(id) = parse_jvmrc(&text) {
                return Some(Resolution::RcFile {
                    root: dir.to_path_buf(),
                    id,
                });
            }
        }
    }

    // 2. projects.json 中最近的项目根祖先
    if let Ok(projects) = store::load_projects(home) {
        if let Some(entry) = projects.nearest_binding(start) {
            if let Ok(id) = JdkId::parse(&entry.jdk) {
                // 命中即刷新 last_used_at，供 GUI 排序展示。shim 每次执行
                // 都会走到这里（零 tokio、零日志的热路径）：距上次刷新超过
                // 1 小时才写盘，避免每次 java 调用都产生一次文件写入；
                // 写回经 update_projects 串行，避免与 bind/unbind 互相覆盖。
                let path = entry.path.clone();
                let _ = store::update_projects(home, |p| {
                    let Some(e) = p.projects.iter_mut().find(|e| e.path == path) else {
                        return Ok(());
                    };
                    let stale = chrono::DateTime::parse_from_rfc3339(&e.last_used_at)
                        .map(|t| {
                            chrono::Utc::now().signed_duration_since(t)
                                > chrono::Duration::hours(1)
                        })
                        .unwrap_or(true);
                    if stale {
                        e.last_used_at = chrono::Utc::now().to_rfc3339();
                    }
                    Ok(())
                });
                return Some(Resolution::Registry {
                    root: PathBuf::from(path),
                    id,
                });
            }
        }
    }

    // 3. 全局默认
    store::read_default(home).ok().flatten().map(Resolution::Global)
}

/// 解析 `.jvmrc` 内容：取首个非空、非注释行的 `vendor@version`。
pub fn parse_jvmrc(text: &str) -> Option<JdkId> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| JdkId::parse(l).ok())
}

/// 在 `dir` 写入 `.jvmrc`（内容为单行 `vendor@version`）。
pub fn write_jvmrc(dir: &Path, id: &JdkId) -> std::io::Result<PathBuf> {
    let path = dir.join(JVMRC_FILE);
    std::fs::write(&path, format!("{id}\n"))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ProjectEntry;

    fn tmp_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_root(dir.path());
        home.ensure_dirs().unwrap();
        (dir, home)
    }

    #[test]
    fn jvmrc_wins_over_everything() {
        let (_d, home) = tmp_home();
        let project = _d.path().join("proj").join("sub");
        std::fs::create_dir_all(&project).unwrap();
        write_jvmrc(
            &_d.path().join("proj"),
            &JdkId::parse("zulu@21.0.1+12").unwrap(),
        )
        .unwrap();
        crate::store::write_default(&home, &JdkId::parse("temurin@17.0.12+7").unwrap()).unwrap();

        let r = resolve(&project, &home).unwrap();
        match r {
            Resolution::RcFile { root, id } => {
                assert_eq!(root, _d.path().join("proj"));
                assert_eq!(id.to_string(), "zulu@21.0.1+12");
            }
            other => panic!("expected RcFile, got {other:?}"),
        }
    }

    #[test]
    fn registry_beats_global_and_picks_nearest() {
        let (_d, home) = tmp_home();
        let deep = _d.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();

        let mut projects = store::Projects::default();
        projects.upsert(ProjectEntry {
            path: _d.path().join("a").to_string_lossy().replace('\\', "/"),
            name: "a".into(),
            jdk: "temurin@21.0.4+7".into(),
            added_at: "t".into(),
            last_used_at: "t".into(),
        });
        projects.upsert(ProjectEntry {
            path: deep.to_string_lossy().replace('\\', "/"),
            name: "b".into(),
            jdk: "temurin@17.0.12+7".into(),
            added_at: "t".into(),
            last_used_at: "t".into(),
        });
        store::save_projects(&home, &projects).unwrap();
        store::write_default(&home, &JdkId::parse("corretto@11.0.24+8").unwrap()).unwrap();

        match resolve(&deep, &home).unwrap() {
            Resolution::Registry { id, .. } => assert_eq!(id.to_string(), "temurin@17.0.12+7"),
            other => panic!("expected Registry, got {other:?}"),
        }
        // a 目录本身命中 a 的绑定
        match resolve(&_d.path().join("a"), &home).unwrap() {
            Resolution::Registry { id, .. } => assert_eq!(id.to_string(), "temurin@21.0.4+7"),
            other => panic!("expected Registry, got {other:?}"),
        }
    }

    #[test]
    fn falls_back_to_global_default() {
        let (_d, home) = tmp_home();
        store::write_default(&home, &JdkId::parse("corretto@11.0.24+8").unwrap()).unwrap();
        let r = resolve(_d.path(), &home).unwrap();
        assert_eq!(r.id().to_string(), "corretto@11.0.24+8");
    }

    #[test]
    fn parse_jvmrc_skips_comments_and_blank() {
        let id = parse_jvmrc("# comment\n\n  temurin@17.0.12+7  \n").unwrap();
        assert_eq!(id.to_string(), "temurin@17.0.12+7");
        assert!(parse_jvmrc("").is_none());
        assert!(parse_jvmrc("# only comment").is_none());
    }

    #[test]
    fn write_jvmrc_creates_file() {
        let d = tempfile::tempdir().unwrap();
        let id = JdkId::parse("zulu@21.0.1+12").unwrap();
        let path = write_jvmrc(d.path(), &id).unwrap();
        assert_eq!(path, d.path().join(".jvmrc"));
        assert_eq!(
            parse_jvmrc(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            id
        );
    }

    #[test]
    fn resolve_refreshes_last_used_at() {
        let (_d, home) = tmp_home();
        let deep = _d.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        let mut projects = store::Projects::default();
        projects.upsert(ProjectEntry {
            path: deep.to_string_lossy().replace('\\', "/"),
            name: "b".into(),
            jdk: "temurin@17.0.12+7".into(),
            added_at: "t".into(),
            last_used_at: "t".into(),
        });
        store::save_projects(&home, &projects).unwrap();

        resolve(&deep, &home).unwrap();
        let updated = store::load_projects(&home).unwrap();
        assert_ne!(
            updated.projects[0].last_used_at, "t",
            "命中登记项目应刷新 last_used_at（GUI 排序用）"
        );
    }
}
