//! 数据目录体检：检测「换根/搬移数据目录」后的悬空引用并自动修复。
//!
//! 背景：数据根（`JVM_HOME`）与设置里的安装/下载目录都设计为可更改，但
//! 多处状态以绝对路径落盘——setting.json 的目录覆盖、current 链接目标、
//! 注册表 JVM_HOME/JAVA_HOME 与用户 PATH。换根后这些引用不会自动随动，坏状态要
//! 等到用户操作时才以报错暴露。本模块集中检测这几类问题；[`repair`] 按
//! 依赖顺序修复：清失效覆盖 → 按默认版本完整重切（重切内部会自愈
//! current、重建 shim、重写环境变量）。

use crate::error::{Error, Result};
use crate::model::JdkId;
use crate::{env, flow, installer, store, Home};
use serde::Serialize;

/// 问题类别（前端按 code 决定图标与是否高亮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCode {
    /// current 缺失、悬空、不是链接或未指向默认版本目录
    CurrentBroken,
    /// default 登记的版本目录缺失或不含 bin/java
    DefaultTargetMissing,
    /// 设置里的目录覆盖指向磁盘上不存在的位置（旧根残留）
    StalePathOverride,
    /// 用户环境变量 JAVA_HOME 未指向本工具的 current
    JavaHomeStale,
    /// 用户环境变量 JVM_HOME 未指向本工具的数据根（workspace）
    JvmHomeStale,
    /// 用户 PATH 缺少本工具的 bin 目录
    PathEntryMissing,
    /// 用户 PATH 存在同义重复条目（展开后指向同一目录）
    PathEntryDuplicate,
}

/// 单个体检问题；`fixable` 表示 [`repair`] 能否自动处理。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthIssue {
    pub code: HealthCode,
    pub message: String,
    pub fixable: bool,
}

/// default 登记版本的可执行 JDK 目录；目录缺失或不含 bin/java 时报错。
fn default_jdk_home(home: &Home, id: &JdkId) -> Result<std::path::PathBuf> {
    let index = store::load_index(home)?;
    let rec = index
        .get(&id.to_string())
        .ok_or_else(|| Error::msg(format!("索引中找不到默认版本 {id}")))?;
    let dir = store::resolve_dir(home, &rec.dir);
    installer::jdk_home_of(&dir)
        .ok_or_else(|| Error::msg(format!("目录 {} 缺失或不含 bin/java", dir.display())))
}

/// 体检当前数据目录，返回发现的问题（无问题返回空列表）。
pub fn check(home: &Home) -> Vec<HealthIssue> {
    let mut out = check_filesystem(home);
    #[cfg(windows)]
    out.extend(check_windows_env(home));
    out
}

/// 文件系统层检查：目录覆盖、default 链路、current 形态。不读注册表，
/// 纯临时目录可测；环境变量检查在 [`check_windows_env`]（仅 Windows）。
pub fn check_filesystem(home: &Home) -> Vec<HealthIssue> {
    let mut out = Vec::new();

    // 设置里的目录覆盖指向不存在位置：换根前的旧绝对路径残留
    if let Ok(settings) = store::load_settings(home) {
        for (label, value) in [
            ("安装目录", settings.install_dir.trim()),
            ("下载目录", settings.download_dir.trim()),
        ] {
            if !value.is_empty() && !std::path::Path::new(value).exists() {
                out.push(HealthIssue {
                    code: HealthCode::StalePathOverride,
                    message: format!(
                        "{label}设置指向不存在的位置 {value}（疑似数据目录移动前的旧路径），修复将清空该设置以跟随数据根目录"
                    ),
                    fixable: true,
                });
            }
        }
    }

    // default 链路：无默认版本时 current 形态无关紧要，直接结束
    let Some(id) = store::read_default(home).ok().flatten() else {
        return out;
    };

    let jdk_home = match default_jdk_home(home, &id) {
        Ok(p) => p,
        Err(e) => {
            out.push(HealthIssue {
                code: HealthCode::DefaultTargetMissing,
                message: format!(
                    "默认版本 {id} 不可用：{e}。请重新安装该版本，或在列表中改选其他默认版本"
                ),
                fixable: false,
            });
            return out;
        }
    };

    // current 形态：缺失 / 悬空 / 指向与默认版本不符都算切换链路断裂。
    // canonicalize 会穿透链接取真实路径，两侧都规范化后即可比较。
    let link = home.current();
    let link_ok = match (
        std::fs::canonicalize(&link),
        std::fs::canonicalize(&jdk_home),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !link_ok {
        out.push(HealthIssue {
            code: HealthCode::CurrentBroken,
            message: format!(
                "current 链接缺失、悬空或未指向默认版本目录 {}（常见于数据目录被整体移动后），修复将自动重建",
                jdk_home.display()
            ),
            fixable: true,
        });
    }

    out
}

/// Windows 环境变量检查：JAVA_HOME 是否指向 current、PATH 是否含 bin。
#[cfg(windows)]
fn check_windows_env(home: &Home) -> Vec<HealthIssue> {
    let mut out = Vec::new();
    // 无默认版本时不校验环境变量：未配置过的全新环境本就没有这些值
    if store::read_default(home).ok().flatten().is_none() {
        return out;
    }
    let link = home.current();
    let current_str = link.to_string_lossy().to_string();
    let root_str = home.root().to_string_lossy().to_string();
    let jvm_home = env::windows::get_user_env("JVM_HOME");
    if jvm_home
        .as_deref()
        .map(|v| !v.eq_ignore_ascii_case(&root_str))
        .unwrap_or(true)
    {
        out.push(HealthIssue {
            code: HealthCode::JvmHomeStale,
            message: format!(
                "用户环境变量 JVM_HOME 为「{}」，未指向数据根 workspace（换根或外部修改后的残留），修复将改写",
                jvm_home.as_deref().unwrap_or("未设置")
            ),
            fixable: true,
        });
    }
    let java_home = env::windows::get_user_env("JAVA_HOME");
    if java_home
        .as_deref()
        .map(|v| !v.eq_ignore_ascii_case(&current_str))
        .unwrap_or(true)
    {
        out.push(HealthIssue {
            code: HealthCode::JavaHomeStale,
            message: format!(
                "用户环境变量 JAVA_HOME 为「{}」，未指向 current 链接（换根或外部修改后的残留），修复将改写",
                java_home.as_deref().unwrap_or("未设置")
            ),
            fixable: true,
        });
    }
    if !env::windows::user_path_contains(&home.bin()) {
        out.push(HealthIssue {
            code: HealthCode::PathEntryMissing,
            message: format!(
                "用户 PATH 缺少本工具目录 {}，终端里 java 等命令将不可用，修复将补上",
                home.bin().display()
            ),
            fixable: true,
        });
    }
    // 同义重复：%VAR% 展开后指向同一目录的条目出现多次（多为修复前
    // 旧版本在变量比较缺失时追加的字面路径）
    let path_text = env::windows::get_user_env("Path").unwrap_or_default();
    let dups = env::windows::duplicate_path_entries(&env::windows::path_entries(&path_text));
    if !dups.is_empty() {
        out.push(HealthIssue {
            code: HealthCode::PathEntryDuplicate,
            message: format!(
                "用户 PATH 存在同义重复条目（{}），指向同一目录的只保留最先出现的一个",
                dups.join("、")
            ),
            fixable: true,
        });
    }
    out
}

/// 修复 [`check`] 发现的问题，返回已执行的动作说明（供界面展示）。
///
/// 顺序：先清失效的目录覆盖（恢复跟随数据根），再按默认版本完整重切。
/// current 残留的普通空目录/被解引用的内容由重切内部的自愈逻辑处理
/// （见 `switcher::rebuild_current_link`），这里不重复实现。
pub fn repair(home: &Home, env: &mut dyn env::UserEnvWriter) -> Result<Vec<String>> {
    let mut actions = Vec::new();

    // 1. 失效的目录覆盖清空，恢复「空 = 跟随数据根」的默认语义
    let mut settings = store::load_settings(home)?;
    let mut dirty = false;
    for (label, field) in [
        ("安装目录", &mut settings.install_dir),
        ("下载目录", &mut settings.download_dir),
    ] {
        let v = field.trim().to_string();
        if !v.is_empty() && !std::path::Path::new(&v).exists() {
            actions.push(format!(
                "已清空{label}覆盖（{v} 不存在），恢复为跟随数据根目录"
            ));
            field.clear();
            dirty = true;
        }
    }
    if dirty {
        store::save_settings(home, &settings)?;
    }

    // 2. 有默认版本：完整重切，一次性修复 current/shim/JAVA_HOME/PATH
    match store::read_default(home)? {
        Some(id) => {
            flow::switch_default_with(home, &id, env)?;
            actions.push(format!(
                "已按默认版本 {id} 重建 current 链接与命令 shim，并写入用户环境变量"
            ));
        }
        None => {
            // 无默认版本：只清理 current 残留（悬空链接/空目录）；
            // 非空目录可能是解引用搬移的真实内容，不自动删。
            let link = home.current();
            if let Ok(meta) = link.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    #[cfg(windows)]
                    std::fs::remove_dir(&link)?;
                    #[cfg(not(windows))]
                    std::fs::remove_file(&link)?;
                    actions.push("已清理悬空的 current 链接（当前无默认版本）".to_string());
                } else if link.is_dir()
                    && std::fs::read_dir(&link)
                        .map(|mut d| d.next().is_none())
                        .unwrap_or(false)
                {
                    std::fs::remove_dir(&link)?;
                    actions.push("已清理残留的 current 空目录（当前无默认版本）".to_string());
                }
            }
        }
    }

    // 3. PATH 同义重复去重（切换里的 append 只保证不新增重复，
    // 已存在的重复在这里统一清理）
    let removed = env.dedupe_path()?;
    if !removed.is_empty() {
        actions.push(format!(
            "已从用户 PATH 移除同义重复条目：{}",
            removed.join("；")
        ));
    }

    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::NoopEnvWriter;
    use crate::store::{InstalledJdk, Settings};

    fn tmp_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_root(dir.path());
        home.ensure_dirs().unwrap();
        (dir, home)
    }

    /// 造一个"可切换"的假 JDK 目录（switch 只校验 bin/java 存在）。
    fn fake_jdk(home: &Home, dir_name: &str) -> std::path::PathBuf {
        let dir = home.versions().join(dir_name);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join(installer::exe_name("java")), b"x").unwrap();
        dir
    }

    fn register(home: &Home, id: &str, dir_name: &str) {
        let mut index = store::Index::default();
        index.upsert(InstalledJdk {
            id: id.to_string(),
            name: id.to_string(),
            vendor: id.split('@').next().unwrap().to_string(),
            version: id.split('@').nth(1).unwrap().to_string(),
            feature: 17,
            lts: true,
            dir: dir_name.to_string(),
            java_version_output: String::new(),
            installed_at: "t".to_string(),
        });
        store::save_index(home, &index).unwrap();
    }

    #[test]
    fn clean_home_has_no_issues() {
        let (_d, home) = tmp_home();
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        fake_jdk(&home, "temurin_17.0.12_7");
        register(&home, "temurin@17.0.12+7", "temurin_17.0.12_7");
        flow::switch_default_with(&home, &id, &mut NoopEnvWriter).unwrap();
        assert!(
            check_filesystem(&home).is_empty(),
            "完整切换后体检应为零问题"
        );
    }

    #[test]
    fn detects_and_repairs_stale_overrides_and_broken_current() {
        let (_d, home) = tmp_home();
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        fake_jdk(&home, "temurin_17.0.12_7");
        register(&home, "temurin@17.0.12+7", "temurin_17.0.12_7");
        flow::switch_default_with(&home, &id, &mut NoopEnvWriter).unwrap();

        // 模拟换根后残留：设置覆盖指向不存在的旧位置 + current 被清掉
        let mut settings = Settings::default();
        settings.install_dir = "C:\\nowhere\\versions".into();
        settings.download_dir = "C:\\nowhere\\downloads".into();
        store::save_settings(&home, &settings).unwrap();
        let issues = check_filesystem(&home);
        assert!(issues
            .iter()
            .any(|i| i.code == HealthCode::StalePathOverride && i.fixable));

        let actions = repair(&home, &mut NoopEnvWriter).unwrap();
        assert_eq!(actions.len(), 3, "清 2 个覆盖 + 1 次重切");
        let s = store::load_settings(&home).unwrap();
        assert!(s.install_dir.is_empty() && s.download_dir.is_empty());
        // 修复后体检归零
        assert!(check_filesystem(&home).is_empty());
    }

    #[test]
    fn default_target_missing_is_reported_unfixable() {
        let (_d, home) = tmp_home();
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        register(&home, "temurin@17.0.12+7", "temurin_17.0.12_7");
        // 索引有登记但目录是空壳（bin/java 缺失）
        std::fs::create_dir_all(home.versions().join("temurin_17.0.12_7")).unwrap();
        store::write_default(&home, &id).unwrap();

        let issues = check_filesystem(&home);
        assert!(issues
            .iter()
            .any(|i| i.code == HealthCode::DefaultTargetMissing && !i.fixable));
        // 修复不应成功重切（目标不可用），但也不应破坏 default 登记
        assert!(repair(&home, &mut NoopEnvWriter).is_err());
        assert_eq!(store::read_default(&home).unwrap().as_ref(), Some(&id));
    }
}
