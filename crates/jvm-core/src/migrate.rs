//! `.jdm` → `.jvm` 的一次性数据迁移。
//!
//! 触发条件（[`migrate_legacy_default_root`]）：未设置 `JVM_HOME`、使用默认
//! 位置、新目录不存在而旧 `.jdm` 存在。整体 rename 后修复三类受影响的引用，
//! 任何一步失败都不阻塞启动（rename 失败时下次启动自动重试）：
//!
//! 1. setting.json 里的绝对路径（install_dir / download_dir 可能指向旧目录）；
//! 2. `current` 联接与用户环境变量 —— junction 与注册表 JAVA_HOME/PATH 都
//!    记录旧绝对路径，改名后悬空/失效；
//! 3. 各登记项目目录里的 `.jdmrc` → `.jvmrc`。
//!
//! bin/ 下的 shim 是完整的二进制拷贝，随目录移动后本身仍可用；迁移末尾按
//! default 版本执行一次完整切换，把 shim 刷新为当前二进制并重写环境变量。

use std::path::Path;

use crate::env::UserEnvWriter;
use crate::home::Home;

/// 默认位置的自动迁移入口：守卫条件不满足时静默返回。
pub fn migrate_legacy_default_root(new_root: &Path) {
    if new_root.exists() {
        return;
    }
    let Some(legacy) = new_root.parent().map(|p| p.join(".jdm")) else {
        return;
    };
    if !legacy.is_dir() {
        return;
    }
    let mut env = crate::env::platform_writer(&new_root.join("logs"));
    migrate_from(&legacy, new_root, env.as_mut());
}

/// 把旧数据目录整体迁移到新根目录。
///
/// `env` 由调用方注入（测试传 [`crate::env::NoopEnvWriter`]，避免改动真实
/// 用户环境变量；生产路径用平台实现，迁移后环境立即生效）。
pub fn migrate_from(legacy: &Path, new_root: &Path, env: &mut dyn UserEnvWriter) {
    // 同盘 rename，瞬间完成；被旧进程占用等情况下失败则放弃本次迁移，
    // 下次启动重试。
    if std::fs::rename(legacy, new_root).is_err() {
        return;
    }
    let home = Home::from_root(new_root);

    remap_settings(&home, legacy, new_root);
    rename_legacy_jdmrcs(&home);

    match crate::store::read_default(&home) {
        Ok(Some(id)) => {
            // 完整切换：写 default → 重建 current 联接 → 刷新 shim →
            // 重写用户 JAVA_HOME / PATH（写入器由调用方注入）。
            if let Err(e) = crate::flow::switch_default_with(&home, &id, env) {
                // 迁移先于日志初始化执行，eprintln 是此时唯一可见通道；
                // 失败不阻塞启动，残留的悬空联接可由 jvm use / doctor 修复。
                eprintln!("jvm 迁移：重建默认切换失败: {e}");
            }
        }
        _ => {
            // 无默认版本：current 联接已悬空，直接摘除（remove_dir 只删联接
            // 本身，不会递归目标内容，理由同 switcher）。
            let _ = std::fs::remove_dir(home.current());
        }
    }
    // PATH 里指向旧目录的条目已成死链，顺手清掉。
    let _ = env.remove_path_entry(&legacy.join("bin"));
}

/// setting.json 中以旧根路径开头的目录字段重映射到新根（含 `/` 分隔写法）。
fn remap_settings(home: &Home, legacy: &Path, new_root: &Path) {
    let Ok(mut settings) = crate::store::load_settings(home) else {
        return;
    };
    let old = legacy.to_string_lossy().to_string();
    let old_slash = old.replace('\\', "/");
    let new = new_root.to_string_lossy().to_string();

    for field in [&mut settings.install_dir, &mut settings.download_dir] {
        let mapped = field.replace(&old, &new).replace(&old_slash, &new);
        if mapped != *field {
            *field = mapped;
        }
    }
    let _ = crate::store::save_settings(home, &settings);
}

/// 登记项目目录里的 `.jdmrc` → `.jvmrc`（目录不存在等情形静默跳过）。
fn rename_legacy_jdmrcs(home: &Home) {
    let Ok(projects) = crate::store::load_projects(home) else {
        return;
    };
    for p in &projects.projects {
        let old = Path::new(&p.path).join(".jdmrc");
        if old.is_file() {
            let _ = std::fs::rename(&old, Path::new(&p.path).join(".jvmrc"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::NoopEnvWriter;
    use crate::store::{InstalledJdk, Index};

    fn write_fake_jdk(dir: &Path) {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("java.exe"), b"fake").unwrap();
        std::fs::write(dir.join("release"), "JAVA_VERSION=\"17.0.1\"\n").unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn migrate_from_moves_home_and_fixups() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join(".jdm");
        let new_root = tmp.path().join(".jvm");
        let home = Home::from_root(&legacy);
        home.ensure_dirs().unwrap();

        let vdir_name = "testv_17.0.1_1";
        let vdir = home.versions().join(vdir_name);
        write_fake_jdk(&vdir);

        let mut index = Index::default();
        index.upsert(InstalledJdk {
            id: "testv@17.0.1+1".into(),
            name: vdir_name.into(),
            vendor: "testv".into(),
            version: "17.0.1+1".into(),
            feature: 17,
            lts: true,
            dir: vdir_name.into(),
            java_version_output: String::new(),
            imported: false,
            external: false,
            installed_at: "t".into(),
        });
        crate::store::save_index(&home, &index).unwrap();
        std::fs::write(home.default_file(), "testv@17.0.1+1").unwrap();

        // setting.json 指向旧根下的目录
        let settings = crate::store::Settings {
            install_dir: home.versions().to_string_lossy().to_string(),
            ..Default::default()
        };
        crate::store::save_settings(&home, &settings).unwrap();

        // 登记项目目录里有旧 .jdmrc
        let proj = tmp.path().join("projectA");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join(".jdmrc"), "testv@17.0.1+1\n").unwrap();
        let mut projects = crate::store::Projects::default();
        projects.projects.push(crate::store::ProjectEntry {
            path: proj.to_string_lossy().to_string(),
            name: "projectA".into(),
            jdk: "testv@17.0.1+1".into(),
            added_at: "t".into(),
            last_used_at: "t".into(),
        });
        crate::store::save_projects(&home, &projects).unwrap();

        // 旧 current 联接指向旧根下的版本（迁移后将悬空）
        junction::create(&vdir, home.current()).unwrap();

        let mut env = NoopEnvWriter;
        migrate_from(&legacy, &new_root, &mut env);

        assert!(!legacy.exists());
        assert!(new_root.join("versions").join(vdir_name).is_dir());

        let migrated = Home::from_root(&new_root);
        let s = crate::store::load_settings(&migrated).unwrap();
        assert_eq!(
            s.install_dir,
            new_root.join("versions").to_string_lossy().to_string()
        );

        assert!(!proj.join(".jdmrc").exists());
        assert!(proj.join(".jvmrc").is_file());

        // current 联接重建到新路径（canonicalize 归一化 verbatim 前缀）
        let vdir_new = new_root.join("versions").join(vdir_name);
        assert!(junction::exists(migrated.current()).unwrap());
        assert_eq!(
            std::fs::canonicalize(migrated.current()).unwrap(),
            std::fs::canonicalize(&vdir_new).unwrap()
        );

        assert_eq!(
            std::fs::read_to_string(migrated.default_file())
                .unwrap()
                .trim(),
            "testv@17.0.1+1"
        );
    }

    #[test]
    fn migrate_skips_when_legacy_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let new_root = tmp.path().join(".jvm");
        migrate_from(&tmp.path().join(".jdm"), &new_root, &mut NoopEnvWriter);
        assert!(!new_root.exists());
    }
}
