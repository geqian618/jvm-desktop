//! 业务编排层：GUI 与 CLI 共用的完整流程。
//!
//! 两个壳（jvm-cli / jvm-tauri）只做输入输出呈现，安装、切换、绑定
//! 的全部步骤收敛在这里，保证行为一致。

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::downloader::{DownloadTask, ProgressFn};
use crate::error::{Error, Result};
use crate::home::Home;
use crate::installer;
use crate::model::{archive_filename, cmp_versions, version_feature, JdkId, Release, VersionRef};
use crate::source::{from_settings, PkgQuery};
use crate::store::{self, Index, InstalledJdk, ProjectEntry, Settings};
use crate::switcher;

/// 拆分用户输入：`temurin@17.0.12+7` → (temurin, Exact)；`17` → (None, Feature)。
pub fn split_id(s: &str) -> (Option<&str>, VersionRef) {
    match s.split_once('@') {
        Some((v, rest)) => (Some(v), VersionRef::parse(rest)),
        None => (None, VersionRef::parse(s)),
    }
}

/// 在已安装列表中按（可选 vendor + 版本引用）宽松匹配，多命中取最新。
pub fn match_installed<'a>(
    index: &'a Index,
    vendor: Option<&str>,
    r#ref: &VersionRef,
) -> Option<&'a InstalledJdk> {
    let mut best: Option<&'a InstalledJdk> = None;
    for rec in &index.installed {
        if let Some(v) = vendor {
            if !rec.vendor.eq_ignore_ascii_case(v) {
                continue;
            }
        }
        if !r#ref.matches(&rec.version) {
            continue;
        }
        match best {
            Some(b) if cmp_versions(&b.version, &rec.version) == std::cmp::Ordering::Greater => {}
            _ => best = Some(rec),
        }
    }
    best
}

/// 把宽松的版本引用解析成远程具体 Release；vendor 缺省为 temurin。
pub async fn resolve_remote_release(settings: &Settings, id: &str) -> Result<Release> {
    let source = from_settings(settings)?;
    let (vendor_arg, r#ref) = split_id(id);
    let vendor = vendor_arg.unwrap_or("temurin").to_string();

    let feature = match &r#ref {
        VersionRef::Feature(f) => *f,
        other => version_feature(match other {
            VersionRef::Exact(v) | VersionRef::Prefix(v) => v.as_str(),
            VersionRef::Feature(_) => unreachable!("covered above"),
        })
        .ok_or_else(|| Error::InvalidId(id.to_string()))?,
    };
    let query = PkgQuery::for_local_machine(&vendor, feature);

    match &r#ref {
        VersionRef::Feature(_) => source
            .latest_for_feature(&query)
            .await?
            .ok_or_else(|| Error::NotFound(id.to_string())),
        VersionRef::Exact(v) => source
            .package_for_exact(&query, v)
            .await?
            .ok_or_else(|| Error::NotFound(id.to_string())),
        VersionRef::Prefix(_) => {
            // 前缀（如 17.0.12）→ 在全部 GA 版本串里取最高的匹配，再查包详情
            let versions = source.version_strings(&vendor, feature).await?;
            let best = versions
                .iter()
                .filter(|v| r#ref.matches(v))
                .max_by(|a, b| cmp_versions(a, b))
                .cloned()
                .ok_or_else(|| Error::NotFound(id.to_string()))?;
            source
                .package_for_exact(&query, &best)
                .await?
                .ok_or_else(|| Error::NotFound(best))
        }
    }
}

/// 下载计划：补全包详情后得到的直链与校验和。
/// 拆出来是为了让调用方（CLI 打印、GUI 日志页）都能先拿到真实直链再下载。
#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub url: String,
    pub expected_sha256: Option<String>,
    pub checksum_url: Option<String>,
    pub archive_type: String,
    pub size: Option<u64>,
}

/// 补全包详情（直链 + 校验和），产出下载计划。直链会记入日志——
/// 下载失败或文件可疑时可据此复现，不再无处可查。
pub async fn plan_download_for_release(
    settings: &Settings,
    release: &Release,
) -> Result<DownloadPlan> {
    let detail =
        if release.package.checksum_sha256.is_some() && release.package.direct_url.is_some() {
            release.clone()
        } else if let Some(uri) = release
            .package
            .info_uri
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            // 首选：按包 id 详情直查（foojay /ids/{id}）。GUI 传入的 Release
            // 带着列表响应里的 info_uri，按 id 补全不依赖版本串匹配——部分包
            // （graalvm/dragonwell 等）用 version= 过滤是命中不到的。
            let source = from_settings(settings)?;
            let det = source.package_detail_by_uri(uri).await?;
            let mut pkg = release.package.clone();
            if det.direct_url.is_some() {
                pkg.direct_url = det.direct_url;
            }
            if det.checksum_sha256.is_some() {
                pkg.checksum_sha256 = det.checksum_sha256;
            }
            if det.checksum_url.is_some() {
                pkg.checksum_url = det.checksum_url;
            }
            Release {
                package: pkg,
                ..release.clone()
            }
        } else {
            // 兜底：版本串精确匹配（CLI 或列表未带 info_uri 的源）。
            let source = from_settings(settings)?;
            let query = PkgQuery::for_local_machine(&release.vendor, release.feature);
            source
                .package_for_exact(&query, &release.version)
                .await?
                .ok_or_else(|| Error::NotFound(release.version.clone()))?
        };

    let url = detail
        .package
        .direct_url
        .clone()
        .ok_or_else(|| Error::msg("下载源未提供可用直链"))?;
    tracing::info!(
        id = %release.id(),
        url = %url,
        size = detail.package.size.unwrap_or(0),
        checksum = detail.package.checksum_sha256.is_some(),
        "已获取下载直链"
    );
    Ok(DownloadPlan {
        url,
        expected_sha256: detail.package.checksum_sha256,
        checksum_url: detail.package.checksum_url,
        archive_type: detail.package.archive_type.clone(),
        size: detail.package.size,
    })
}

/// 按下载计划下载归档到下载目录（校验通过才落最终文件名）。
pub async fn download_with_plan(
    home: &Home,
    settings: &Settings,
    release: &Release,
    plan: DownloadPlan,
    progress: Option<Arc<ProgressFn>>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<std::path::PathBuf> {
    let dest = home
        .downloads()
        .join(archive_filename(&release.id(), &plan.archive_type));

    crate::downloader::download(
        DownloadTask {
            client: crate::net::build_client(settings)?,
            url: plan.url,
            dest_file: dest,
            expected_sha256: plan.expected_sha256,
            checksum_url: plan.checksum_url,
            expected_size: plan.size,
            // 并发 4：实测部分下载源（如 download.java.net）按"每 IP 总量"
            // 限速——16 连接并发实测总速度与单连接几乎相同（~300KB/s），
            // 只会把额度切成细条、把每条压到看门狗阈值之下诱发重连。
            // 4 连接在"每连接限速"与"每 IP 限速"两类路径上都是安全档。
            concurrency: 4,
            cancel,
        },
        progress,
    )
    .await
}

/// 补全包详情并下载归档；仅下载与完整安装共用。
pub async fn download_for_release(
    home: &Home,
    settings: &Settings,
    release: &Release,
    progress: Option<Arc<ProgressFn>>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<std::path::PathBuf> {
    let plan = plan_download_for_release(settings, release).await?;
    download_with_plan(home, settings, release, plan, progress, cancel).await
}

/// 安装已下载的归档：解压 → 验证 → 写索引。`dir_name` 允许 GUI 自定义
/// 目录名（如 `{distribution}@{javaVersion}`），缺省用 [`JdkId::to_dir_name`]。
pub fn install_downloaded(
    home: &Home,
    release: &Release,
    archive: &Path,
    dir_name: Option<&str>,
) -> Result<InstalledJdk> {
    let jdk_id = release.id();
    // 仅作快速失败（免白做解压验证）；登记写回前会在锁内重读最新索引
    if store::load_index(home)?.get(&jdk_id.to_string()).is_some() {
        return Err(Error::AlreadyInstalled(jdk_id.to_string()));
    }

    // GUI 目录名包含 '+'（如 aoj-16.0.1+1），Windows 目录名合法，无需消毒；
    // 仅拒绝路径分隔符与保留字符。
    let dir = match dir_name {
        Some(name) => {
            let name = name.trim();
            if name.is_empty() || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
                return Err(Error::msg(format!("非法的安装目录名: {name}")));
            }
            name.to_string()
        }
        None => jdk_id.to_dir_name(),
    };

    let versions = home.versions();
    std::fs::create_dir_all(&versions)?;
    let dest = versions.join(&dir);
    if dest.exists() {
        return Err(Error::AlreadyInstalled(format!(
            "{jdk_id}（目录 {dir} 已存在）"
        )));
    }

    let outcome = installer::install_to_dir(archive, &dest, &installer::run_java_version)?;

    let rec = InstalledJdk {
        id: jdk_id.to_string(),
        name: dir,
        vendor: jdk_id.vendor.clone(),
        version: jdk_id.version.clone(),
        feature: release.feature,
        lts: release.lts,
        dir: dest
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        java_version_output: outcome.java_version_output,
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    // 锁内重读最新索引再登记：安装（下载+解压）耗时期间其他并发任务
    // 可能已写入，沿用开头的快照整体回写会抹掉它们的登记。
    store::update_index(home, |index| {
        index.upsert(rec.clone());
        Ok(())
    })?;
    Ok(rec)
}

/// 下载并安装一个 Release（CLI 完整流程）；返回索引记录。
pub async fn install_release(
    home: &Home,
    settings: &Settings,
    release: &Release,
    progress: Option<Arc<ProgressFn>>,
) -> Result<InstalledJdk> {
    let plan = plan_download_for_release(settings, release).await?;
    install_release_with_plan(home, settings, release, plan, progress).await
}

/// [`install_release`] 的可拆分变体：调用方先经 [`plan_download_for_release`]
/// 拿到下载计划（CLI 借此在终端打印真实直链），再传入执行。
pub async fn install_release_with_plan(
    home: &Home,
    settings: &Settings,
    release: &Release,
    plan: DownloadPlan,
    progress: Option<Arc<ProgressFn>>,
) -> Result<InstalledJdk> {
    home.ensure_dirs()?;
    let jdk_id = release.id();
    // 仅作快速失败（免白做下载解压）；登记写回前会在锁内重读最新索引
    if store::load_index(home)?.get(&jdk_id.to_string()).is_some() {
        return Err(Error::AlreadyInstalled(format!(
            "{jdk_id}（如需切换: jvm use {jdk_id}）"
        )));
    }

    let downloaded = download_with_plan(home, settings, release, plan, progress, None).await?;
    install_downloaded(home, release, &downloaded, None)
}

/// 启用某个已安装（索引已登记）的 JDK 目录。
pub fn use_dir(home: &Home, dir: &Path) -> Result<JdkId> {
    let mut env_writer = crate::env::platform_writer(&home.logs());
    use_dir_with(home, dir, env_writer.as_mut())
}

/// [`use_dir`] 的可注入变体：测试传 `NoopEnvWriter` 以免改动真实系统环境。
pub fn use_dir_with(
    home: &Home,
    dir: &Path,
    env: &mut dyn crate::env::UserEnvWriter,
) -> Result<JdkId> {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if installer::jdk_home_of(&dir).is_none() {
        return Err(Error::msg(format!(
            "{} 不是有效的 JDK 主目录（缺 bin/java）",
            dir.display()
        )));
    }

    let index = store::load_index(home)?;
    let id = index
        .installed
        .iter()
        .find(|r| {
            crate::store::path_key(&store::resolve_dir(home, &r.dir))
                == crate::store::path_key(&dir)
        })
        .map(|rec| rec.jdk_id())
        .ok_or_else(|| {
            Error::msg(format!(
                "{} 不是本工具安装的 JDK（未登记索引）",
                dir.display()
            ))
        })?;

    switch_default_with(home, &id, env)?;
    tracing::info!(id = %id, dir = %dir.display(), "已启用 JDK（default/current 链接/shim 已更新）");
    Ok(id)
}

/// 卸载：守卫（默认/被绑定拒绝）→ 删目录 → 删索引。
pub fn uninstall(home: &Home, id: &JdkId) -> Result<()> {
    switcher::ensure_uninstallable(home, id)?;
    let index = store::load_index(home)?;
    let rec = index
        .get(&id.to_string())
        .ok_or_else(|| Error::NotFound(id.to_string()))?
        .clone();
    tracing::info!(id = %id, dir = %rec.dir, "开始卸载 JDK");
    let dir = store::resolve_dir(home, &rec.dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    // 锁内重读-移除-写回，避免与并发安装/登记互相覆盖
    store::update_index(home, |index| {
        index.remove(&id.to_string());
        Ok(())
    })?;
    Ok(())
}

/// 切换全局默认（含环境变量与 shim 重建）；返回切换结果，hints 仅在
/// 用户环境变量实际变更（首次配置/修复）时携带"重开终端"提示。
pub fn switch_default(home: &Home, id: &JdkId) -> Result<crate::switcher::SwitchOutcome> {
    let mut env_writer = crate::env::platform_writer(&home.logs());
    switch_default_with(home, id, env_writer.as_mut())
}

/// [`switch_default`] 的可注入变体：测试传 `NoopEnvWriter` 以免改动真实系统环境。
pub fn switch_default_with(
    home: &Home,
    id: &JdkId,
    env: &mut dyn crate::env::UserEnvWriter,
) -> Result<crate::switcher::SwitchOutcome> {
    switcher::switch_to(home, id, env)
}

/// 绑定项目目录：写 `.jvmrc` 并登记 projects.json。
pub fn bind_project(home: &Home, dir: &Path, id: &JdkId) -> Result<bool> {
    crate::project::write_jvmrc(dir, id)?;
    let now = chrono::Utc::now().to_rfc3339();
    // 写回经全局锁串行：避免与 resolve 的 last_used_at 刷新等并发写者互相覆盖
    let is_new = store::update_projects(home, |projects| {
        Ok(projects.upsert(ProjectEntry {
            path: dir.to_string_lossy().replace('\\', "/"),
            name: dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "project".to_string()),
            jdk: id.to_string(),
            added_at: now.clone(),
            last_used_at: now,
        }))
    })?;
    tracing::info!(dir = %dir.display(), id = %id, new = is_new, "项目关联已更新");
    Ok(is_new)
}

/// 取消默认版本（GUI/CLI 共用）：摘 current 链接、删 shim、清理环境变量。
pub fn unset_default(home: &Home) -> Result<bool> {
    let mut env_writer = crate::env::platform_writer(&home.logs());
    unset_default_with(home, env_writer.as_mut())
}

/// [`unset_default`] 的可注入变体：测试传 `NoopEnvWriter`。
pub fn unset_default_with(home: &Home, env: &mut dyn crate::env::UserEnvWriter) -> Result<bool> {
    switcher::unset_default(home, env)
}

/// 解除项目目录绑定（保留磁盘上的 .jvmrc 删除与否交由调用方）。
pub fn unbind_project(home: &Home, dir: &Path) -> Result<bool> {
    let removed = store::update_projects(home, |projects| Ok(projects.remove_by_path(dir)))?;
    tracing::info!(dir = %dir.display(), removed, "项目关联解除请求处理完毕");
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::NoopEnvWriter;
    use crate::model::VersionRef;

    fn tmp_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_root(dir.path());
        home.ensure_dirs().unwrap();
        (dir, home)
    }

    fn fake_jdk_at(dir: &Path, version: &str) {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join(installer::exe_name("java")), b"fake").unwrap();
        std::fs::write(
            dir.join("release"),
            format!("JAVA_VERSION=\"{version}\"\nIMPLEMENTOR=\"Test Vendor\"\n"),
        )
        .unwrap();
    }

    fn rec_with(id: &str, vendor: &str, version: &str, feature: u32, dir: &str) -> InstalledJdk {
        InstalledJdk {
            id: id.to_string(),
            name: dir.to_string(),
            vendor: vendor.to_string(),
            version: version.to_string(),
            feature,
            lts: true,
            dir: dir.to_string(),
            java_version_output: String::new(),
            installed_at: "t".into(),
        }
    }

    #[test]
    fn split_id_splits_vendor_and_ref() {
        let (v, r) = split_id("temurin@17.0.12+7");
        assert_eq!(v, Some("temurin"));
        assert_eq!(r, VersionRef::Exact("17.0.12+7".into()));

        let (v, r) = split_id("17");
        assert_eq!(v, None);
        assert_eq!(r, VersionRef::Feature(17));

        let (v, r) = split_id("zulu@21");
        assert_eq!(v, Some("zulu"));
        assert_eq!(r, VersionRef::Feature(21));

        let (v, r) = split_id("17.0.12");
        assert_eq!(v, None);
        assert_eq!(r, VersionRef::Prefix("17.0.12".into()));
    }

    #[test]
    fn match_installed_picks_newest_and_respects_vendor() {
        let mut index = Index::default();
        index.upsert(rec_with(
            "temurin@17.0.12+7",
            "temurin",
            "17.0.12+7",
            17,
            "a",
        ));
        index.upsert(rec_with(
            "temurin@17.0.20.1+1",
            "temurin",
            "17.0.20.1+1",
            17,
            "b",
        ));
        index.upsert(rec_with("zulu@17.0.12+7", "zulu", "17.0.12+7", 17, "c"));
        index.upsert(rec_with("zulu@21.0.1+12", "zulu", "21.0.1+12", 21, "d"));

        // 主版本宽松匹配：多命中取最新（跨发行商）
        let best = match_installed(&index, None, &VersionRef::parse("17")).unwrap();
        assert_eq!(best.version, "17.0.20.1+1");

        // vendor 过滤不区分大小写
        let best = match_installed(&index, Some("ZULU"), &VersionRef::parse("21")).unwrap();
        assert_eq!(best.id, "zulu@21.0.1+12");

        // 前缀匹配
        let best = match_installed(&index, Some("temurin"), &VersionRef::parse("17.0.12")).unwrap();
        assert_eq!(best.version, "17.0.12+7");

        // 未命中
        assert!(match_installed(&index, Some("corretto"), &VersionRef::parse("17")).is_none());
        assert!(match_installed(&index, None, &VersionRef::parse("11")).is_none());
    }

    #[test]
    fn install_downloaded_rejects_bad_dir_name_and_duplicate() {
        let (_d, home) = tmp_home();
        let release = Release {
            vendor: "temurin".into(),
            version: "17.0.12+7".into(),
            feature: 17,
            term: "LTS".into(),
            lts: true,
            package: crate::model::RemotePackage {
                os: "windows".into(),
                arch: "x64".into(),
                archive_type: "zip".into(),
                size: None,
                filename: "jdk.zip".into(),
                distribution_version: "17.0.12".into(),
                image_type: "jdk".into(),
                direct_url: None,
                info_uri: None,
                checksum_sha256: None,
                checksum_url: None,
            },
        };
        let archive = _d.path().join("whatever.zip");
        std::fs::write(&archive, b"zip").unwrap();

        // 非法目录名（含路径分隔符等保留字符）在解压前就被拒绝
        for bad in ["a/b", "a\\b", "a:b", "", "  "] {
            assert!(
                install_downloaded(&home, &release, &archive, Some(bad)).is_err(),
                "目录名 `{bad}` 应被拒绝"
            );
        }

        // 索引中已有同 id 记录 → AlreadyInstalled（不触碰归档）
        let mut index = store::load_index(&home).unwrap();
        index.upsert(rec_with(
            "temurin@17.0.12+7",
            "temurin",
            "17.0.12+7",
            17,
            "temurin_17.0.12_7",
        ));
        store::save_index(&home, &index).unwrap();
        assert!(matches!(
            install_downloaded(&home, &release, &archive, None),
            Err(Error::AlreadyInstalled(_))
        ));
    }

    #[test]
    fn concurrent_index_writers_keep_each_others_records() {
        // 回归：GUI 并发安装多个 JDK 时，各任务持旧快照整体回写 index.json，
        // 后写者会抹掉先写者的登记（表现为"未在管理列表中找到"）。
        // update_index 的全局锁必须保证 8 个并发写者的记录全部留存。
        let (_d, home) = tmp_home();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let home = home.clone();
                std::thread::spawn(move || {
                    store::update_index(&home, |index| {
                        index.upsert(rec_with(
                            &format!("vendor{i}@17.0.{i}+1"),
                            &format!("vendor{i}"),
                            &format!("17.0.{i}+1"),
                            17,
                            &format!("vendor{i}_17.0.{i}_1"),
                        ));
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let index = store::load_index(&home).unwrap();
        for i in 0..8 {
            assert!(
                index.get(&format!("vendor{i}@17.0.{i}+1")).is_some(),
                "vendor{i} 的登记在并发写回后丢失"
            );
        }
    }

    #[test]
    fn uninstall_removes_internal_dir_and_index_entry() {
        let (_d, home) = tmp_home();
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        let dir = home.version_dir(&id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("marker"), b"x").unwrap();
        let mut index = store::load_index(&home).unwrap();
        index.upsert(rec_with(
            &id.to_string(),
            "temurin",
            "17.0.12+7",
            17,
            &id.to_dir_name(),
        ));
        store::save_index(&home, &index).unwrap();

        // 默认版本受保护
        store::write_default(&home, &id).unwrap();
        assert!(matches!(uninstall(&home, &id), Err(Error::InUse(_))));
        store::write_default(&home, &JdkId::new("zulu", "21")).unwrap();

        uninstall(&home, &id).unwrap();
        assert!(!dir.exists(), "内部版本目录应被删除");
        assert!(store::load_index(&home)
            .unwrap()
            .get(&id.to_string())
            .is_none());
    }

    #[test]
    fn bind_and_unbind_project() {
        let (_d, home) = tmp_home();
        let proj = _d.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();

        assert!(bind_project(&home, &proj, &id).unwrap(), "首次绑定是新项目");
        // .jvmrc 已写入
        assert_eq!(
            crate::project::parse_jvmrc(&std::fs::read_to_string(proj.join(".jvmrc")).unwrap())
                .unwrap(),
            id
        );
        // projects.json 已登记
        let projects = store::load_projects(&home).unwrap();
        assert_eq!(projects.projects.len(), 1);
        assert_eq!(projects.projects[0].jdk, id.to_string());

        // 重复绑定同一目录：更新而非新增
        let other = JdkId::parse("zulu@21.0.1+12").unwrap();
        assert!(!bind_project(&home, &proj, &other).unwrap());
        assert_eq!(store::load_projects(&home).unwrap().projects.len(), 1);
        assert_eq!(
            crate::project::parse_jvmrc(&std::fs::read_to_string(proj.join(".jvmrc")).unwrap())
                .unwrap(),
            other
        );

        assert!(unbind_project(&home, &proj).unwrap());
        assert!(!unbind_project(&home, &proj).unwrap(), "重复解除返回 false");
        assert!(store::load_projects(&home).unwrap().projects.is_empty());
        // .jvmrc 文件保留（是否删除由调用方决定）
        assert!(proj.join(".jvmrc").exists());
    }

    #[test]
    fn use_dir_switches_existing_internal_version() {
        let (_d, home) = tmp_home();
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        let dir = home.version_dir(&id);
        fake_jdk_at(&dir, "17.0.12");
        let mut index = store::load_index(&home).unwrap();
        index.upsert(rec_with(
            &id.to_string(),
            "temurin",
            "17.0.12+7",
            17,
            &id.to_dir_name(),
        ));
        store::save_index(&home, &index).unwrap();

        let mut env = NoopEnvWriter;
        let got = use_dir_with(&home, &dir, &mut env).unwrap();
        assert_eq!(got, id);
        assert_eq!(store::read_default(&home).unwrap().unwrap(), id);
        // 索引不新增条目
        assert_eq!(store::load_index(&home).unwrap().installed.len(), 1);
        // current 链接与 shim 已就绪
        assert!(home.current().symlink_metadata().is_ok());
        assert!(home.bin().join(installer::exe_name("java")).is_file());
    }
}
