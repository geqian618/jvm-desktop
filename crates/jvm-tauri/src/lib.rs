//! Tauri 命令层：把 jvm-core 的能力暴露给前端。
//!
//! 薄壳原则：这里只做参数/DTO 转换、缓存与事件推送，业务全部在
//! `jvm_core::flow` / `scan` / `source` 中。

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jvm_core::model::{archive_filename, Release, RemotePackage};
use jvm_core::source::{DistributionInfo, MajorVersion, from_settings};
use jvm_core::store::{InstalledJdk, ProjectEntry, Settings};
use jvm_core::{flow, scan, shim, store, switcher, Home};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

/// 元数据/包列表缓存 TTL。
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

struct AppState {
    /// 包列表缓存：key = registry|distribution|os|arch|archive|proxy
    pkg_cache: Mutex<HashMap<String, (Instant, Vec<Release>)>>,
    meta_cache: Mutex<HashMap<String, (Instant, CachedMeta)>>,
    /// 安装/下载任务取消开关：key = taskId（即 release id）
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

struct CachedMeta {
    distributions: Vec<DistributionInfo>,
    majors: Vec<MajorVersion>,
}

fn estr(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn app_home() -> Result<Home, String> {
    let home = Home::configured();
    home.ensure_dirs().map_err(estr)?;
    Ok(home)
}

// ---------- 事件负载 ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallProgress {
    task_id: String,
    /// download / extract / done / error / cancelled
    stage: String,
    downloaded: u64,
    total: u64,
    percent: f64,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppLog {
    ts: u64,
    /// info / success / warn / error
    level: String,
    message: String,
}

fn log_evt(app: &AppHandle, level: &str, message: impl Into<String>) {
    let _ = app.emit(
        "app-log",
        AppLog {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            level: level.to_string(),
            message: message.into(),
        },
    );
}

// ---------- DTO ----------

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDto {
    /// 唯一键：`<vendor>@<javaVersion>`，同时是安装任务的 taskId。
    id: String,
    distribution: String,
    java_version: String,
    distribution_version: String,
    /// 包类型：jdk / jre
    package_type: String,
    term_of_support: String,
    archive_type: String,
    filename: String,
    size: u64,
    directly_downloadable: bool,
    /// 包详情地址（foojay `/ids/{id}`），安装时按 id 直查直链与校验和。
    #[serde(default)]
    info_uri: Option<String>,
}

impl ReleaseDto {
    fn from_release(r: &Release) -> Self {
        Self {
            id: r.id().to_string(),
            distribution: r.vendor.clone(),
            java_version: r.version.clone(),
            distribution_version: r.package.distribution_version.clone(),
            package_type: r.package.image_type.clone(),
            term_of_support: r.term.clone(),
            archive_type: r.package.archive_type.clone(),
            filename: r.package.filename.clone(),
            size: r.package.size.unwrap_or(0),
            directly_downloadable: r.package.direct_url.is_some() || r.package.info_uri.is_some(),
            info_uri: r.package.info_uri.clone(),
        }
    }

    /// GUI 回传的列表项 → Release。直链/校验和留空，由下载阶段按
    /// `info_uri` 直查补全——不能按版本串重新解析：部分包（graalvm、
    /// dragonwell 等）的 `version=` 过滤命中不到，会误报"未找到版本"。
    fn to_release(&self) -> Option<Release> {
        let feature = jvm_core::model::version_feature(&self.java_version)?;
        Some(Release {
            vendor: self.distribution.clone(),
            version: self.java_version.clone(),
            feature,
            term: self.term_of_support.to_uppercase(),
            lts: self.term_of_support.eq_ignore_ascii_case("lts"),
            package: RemotePackage {
                os: jvm_core::model::current_os().to_string(),
                arch: jvm_core::model::current_arch().to_string(),
                archive_type: self.archive_type.clone(),
                size: (self.size > 0).then_some(self.size),
                filename: self.filename.clone(),
                distribution_version: self.distribution_version.clone(),
                image_type: self.package_type.clone(),
                direct_url: None,
                info_uri: self.info_uri.clone(),
                checksum_sha256: None,
                checksum_url: None,
            },
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalJdkDto {
    dir: String,
    name: String,
    java_version: Option<String>,
    vendor: Option<String>,
    /// 索引记录 id（vendor@version）；目录未登记时为空
    id: Option<String>,
    /// 安装日期（索引 installed_at 转本地时区 YYYY-MM-DD；未登记时为空）
    installed_at: Option<String>,
    in_install_dir: bool,
    is_current: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlatformDto {
    os: String,
    arch: String,
}

// ---------- 配置 ----------

#[tauri::command]
async fn get_config() -> Result<Settings, String> {
    let home = app_home()?;
    store::load_settings(&home).map_err(estr)
}

#[tauri::command]
async fn save_config(app: AppHandle, config: Settings) -> Result<(), String> {
    let home = app_home()?;
    store::save_settings(&home, &config).map_err(estr)?;
    // 关键变更记入文件日志便于排查（不记录 API Key 等敏感字段）。
    tracing::info!(
        registry = %config.registry,
        install_dir = %config.install_dir,
        download_dir = %config.download_dir,
        proxy_set = !config.proxy.trim().is_empty(),
        "设置已保存"
    );
    log_evt(&app, "success", "设置已保存");
    Ok(())
}

/// 临时诊断通道：把 webview 侧的错误文本转发到 dev 终端。
#[tauri::command]
async fn debug_log(message: String) -> Result<(), String> {
    eprintln!("[webview] {message}");
    Ok(())
}

#[tauri::command]
async fn get_platform() -> PlatformDto {
    PlatformDto {
        os: jvm_core::model::current_os().to_string(),
        arch: jvm_core::model::current_arch().to_string(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PathDefaultsDto {
    install_dir: String,
    download_dir: String,
}

/// 设置里留空时实际生效的默认目录（基于 detect，不套用设置覆盖）。
#[tauri::command]
async fn get_default_paths() -> Result<PathDefaultsDto, String> {
    let home = Home::detect();
    Ok(PathDefaultsDto {
        install_dir: home.versions().to_string_lossy().into_owned(),
        download_dir: home.downloads().to_string_lossy().into_owned(),
    })
}

// ---------- 数据源 ----------

#[tauri::command]
async fn get_distributions(state: State<'_, SharedState>) -> Result<Vec<DistributionInfo>, String> {
    let home = app_home()?;
    let settings = store::load_settings(&home).map_err(estr)?;
    let key = format!("dist|{}", settings.registry);
    if let Some((t, meta)) = state.meta_cache.lock().unwrap().get(&key) {
        if t.elapsed() < CACHE_TTL {
            return Ok(meta.distributions.clone());
        }
    }
    let list = from_settings(&settings)
        .map_err(estr)?
        .distributions()
        .await
        .map_err(estr)?;
    // 必须 insert 新条目（并重置时间戳）：只 get_mut 更新已存在条目时，
    // 首次调用后缓存里永远没有这个 key，缓存形同虚设。
    state.meta_cache.lock().unwrap().insert(
        key,
        (
            Instant::now(),
            CachedMeta {
                distributions: list.clone(),
                majors: Vec::new(),
            },
        ),
    );
    Ok(list)
}

#[tauri::command]
async fn get_major_versions(state: State<'_, SharedState>) -> Result<Vec<MajorVersion>, String> {
    let home = app_home()?;
    let settings = store::load_settings(&home).map_err(estr)?;
    let key = format!("majors|{}", settings.registry);
    if let Some((t, meta)) = state.meta_cache.lock().unwrap().get(&key) {
        if t.elapsed() < CACHE_TTL {
            return Ok(meta.majors.clone());
        }
    }
    let list = from_settings(&settings)
        .map_err(estr)?
        .major_versions()
        .await
        .map_err(estr)?;
    // 同 get_distributions：insert 新条目并重置时间戳。
    state.meta_cache.lock().unwrap().insert(
        key,
        (
            Instant::now(),
            CachedMeta {
                distributions: Vec::new(),
                majors: list.clone(),
            },
        ),
    );
    Ok(list)
}

/// 一次拉取该发行商在当前平台/归档格式下的全部 GA 包（不带 version 参数）。
#[tauri::command]
async fn get_all_releases(
    app: AppHandle,
    state: State<'_, SharedState>,
    distribution: String,
    operating_system: String,
    architecture: String,
    archive_type: String,
) -> Result<Vec<ReleaseDto>, String> {
    let home = app_home()?;
    let settings = store::load_settings(&home).map_err(estr)?;
    let key = format!(
        "{}|{}|{}|{}|{}|{}",
        settings.registry, distribution, operating_system, architecture, archive_type, settings.proxy
    );
    if let Some((t, list)) = state.pkg_cache.lock().unwrap().get(&key) {
        if t.elapsed() < CACHE_TTL {
            return Ok(list.iter().map(ReleaseDto::from_release).collect());
        }
    }
    let start = Instant::now();
    let list = from_settings(&settings)
        .map_err(estr)?
        .all_packages(&distribution, &operating_system, &architecture, &archive_type)
        .await
        .map_err(estr)?;
    tracing::info!(vendor = %distribution, n = list.len(), elapsed = ?start.elapsed(), "已加载发行版包列表");
    state
        .pkg_cache
        .lock()
        .unwrap()
        .insert(key, (Instant::now(), list.clone()));
    log_evt(
        &app,
        "info",
        format!(
            "已加载 {distribution} 的 {} 个包（{operating_system}/{architecture}/{archive_type}）",
            list.len()
        ),
    );
    Ok(list.iter().map(ReleaseDto::from_release).collect())
}

// ---------- 本地 JDK ----------

/// 扫描 + DTO 组装（同步重 IO：目录遍历与可能的 java -version 子进程）。
/// 独立成同步函数以便在 [`tauri::async_runtime::spawn_blocking`] 中执行，
/// 避免阻塞 tokio worker。
fn build_local_jdk_dtos(home: &Home) -> Result<Vec<LocalJdkDto>, String> {
    let found = scan::scan_local(home);

    let current_key = store::read_default(home)
        .ok()
        .flatten()
        .and_then(|id| {
            store::load_index(home)
                .ok()?
                .get(&id.to_string())
                .map(|rec| store::path_key(&store::resolve_dir(home, &rec.dir)))
        });

    // 规范化路径 → 索引记录（id 供前端判断项目关联占用，日期供列表展示）
    let index_by_path: std::collections::HashMap<String, (String, Option<String>)> =
        store::load_index(home)
            .unwrap_or_default()
            .installed
            .iter()
            .map(|rec| {
                let key = store::path_key(&store::resolve_dir(home, &rec.dir));
                (key, (rec.id.clone(), rec.installed_date()))
            })
            .collect();

    Ok(found
        .into_iter()
        .map(|j| {
            let key = store::path_key(std::path::Path::new(&j.dir));
            let rec = index_by_path.get(&key);
            LocalJdkDto {
                is_current: current_key.as_deref() == Some(key.as_str()),
                id: rec.map(|(id, _)| id.clone()),
                installed_at: rec.and_then(|(_, d)| d.clone()),
                dir: j.dir,
                name: j.name,
                java_version: j.java_version,
                vendor: j.vendor,
                in_install_dir: j.in_install_dir,
            }
        })
        .collect())
}

#[tauri::command]
async fn get_local_jdks() -> Result<Vec<LocalJdkDto>, String> {
    let home = app_home()?;
    tauri::async_runtime::spawn_blocking(move || build_local_jdk_dtos(&home))
        .await
        .map_err(estr)?
}

#[tauri::command]
async fn get_current_java_home() -> Result<Option<String>, String> {
    let home = app_home()?;
    if store::read_default(&home).map_err(estr)?.is_some() {
        return Ok(Some(home.current().to_string_lossy().into_owned()));
    }
    Ok(std::env::var("JAVA_HOME").ok())
}

/// 启用某目录的 JDK（内部已装版本或外部 JDK）。
#[tauri::command]
async fn use_jdk(app: AppHandle, dir: String) -> Result<(), String> {
    let home = app_home()?;
    // junction 重建 + shim 复制 + 注册表写入均为阻塞 IO：放线程池执行
    let id = tauri::async_runtime::spawn_blocking(move || {
        flow::use_dir(&home, std::path::Path::new(&dir))
    })
    .await
    .map_err(estr)?
    .map_err(estr)?;
    log_evt(&app, "success", format!("已设为默认 JDK: {id}"));
    Ok(())
}

#[tauri::command]
async fn uninstall_jdk(app: AppHandle, dir: String) -> Result<(), String> {
    let home = app_home()?;
    // 递归删除目录是数百 MB 级阻塞 IO：放线程池执行
    let rec = tauri::async_runtime::spawn_blocking(move || -> Result<store::InstalledJdk, String> {
        let index = store::load_index(&home).map_err(estr)?;
        let key = store::path_key(std::path::Path::new(&dir));
        let rec = index
            .installed
            .iter()
            .find(|r| store::path_key(&store::resolve_dir(&home, &r.dir)) == key)
            .cloned()
            .ok_or_else(|| format!("未在管理列表中找到 {dir}"))?;
        flow::uninstall(&home, &rec.jdk_id()).map_err(estr)?;
        Ok(rec)
    })
    .await
    .map_err(estr)??;
    log_evt(&app, "success", format!("已卸载 JDK: {}", rec.id));
    Ok(())
}

#[tauri::command]
async fn open_dir(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(estr)
}

// ---------- 安装 / 仅下载 ----------

fn spawn_progress_emitter(
    app: AppHandle,
    task_id: String,
) -> Arc<jvm_core::downloader::ProgressFn> {
    // 进度事件节流：100ms 一条，避免高频事件打爆 IPC。
    let last = Arc::new(Mutex::new(Instant::now() - Duration::from_millis(1000)));
    Arc::new(move |done: u64, total: u64| {
        let mut guard = last.lock().unwrap();
        if guard.elapsed() < Duration::from_millis(100) && done < total {
            return;
        }
        *guard = Instant::now();
        drop(guard);
        let percent = if total > 0 { done as f64 / total as f64 * 100.0 } else { 0.0 };
        let _ = app.emit(
            "install-progress",
            InstallProgress {
                task_id: task_id.clone(),
                stage: "download".to_string(),
                downloaded: done,
                total,
                percent,
                message: String::new(),
            },
        );
    })
}

fn emit_stage(app: &AppHandle, task_id: &str, stage: &str, message: &str) {
    let _ = app.emit(
        "install-progress",
        InstallProgress {
            task_id: task_id.to_string(),
            stage: stage.to_string(),
            downloaded: 0,
            total: 0,
            percent: 0.0,
            message: message.to_string(),
        },
    );
}

/// 登记新任务的取消 token；同 task_id 已有任务在途时返回 false（拒绝
/// 重复发起）。下载与安装是两个入口且共用 release.id 作为任务号，不加
/// 防护会出现并发写同一 .part 文件、cancels 表项互相覆盖的问题。
fn register_task(state: &AppState, task_id: &str, token: Arc<AtomicBool>) -> bool {
    use std::collections::hash_map::Entry;
    match state.cancels.lock().unwrap().entry(task_id.to_string()) {
        Entry::Occupied(_) => false,
        Entry::Vacant(v) => {
            v.insert(token);
            true
        }
    }
}

async fn run_install_or_download(
    app: AppHandle,
    state: &AppState,
    task_id: String,
    id: String,
    dto_release: Option<Release>,
    dir_name: Option<String>,
    token: Arc<AtomicBool>,
) {
    let verb = if dir_name.is_some() { "安装" } else { "下载" };
    log_evt(&app, "info", format!("开始{verb}: {id}"));

    let result: Result<(), jvm_core::Error> = async {
        let home = Home::configured();
        home.ensure_dirs()?;
        let settings = store::load_settings(&home)?;
        // GUI 传入的列表项优先（带 info_uri，按包 id 直查详情）；
        // 只有 CLI 风格的纯 id 字符串才走版本串解析。
        let release = match dto_release {
            Some(r) => r,
            None => flow::resolve_remote_release(&settings, &id).await?,
        };

        // 先报「准备中」：解析下载直链需要一次网络请求，复用已下载压缩包时
        // 不会再进入 download 阶段，避免 UI 闪现下载进度条
        emit_stage(&app, &task_id, "prepare", "");
        let plan = flow::plan_download_for_release(&settings, &release).await?;
        let archive_path = home
            .downloads()
            .join(archive_filename(&release.id(), &plan.archive_type));
        if !archive_path.exists() {
            emit_stage(&app, &task_id, "download", "");
        } else {
            log_evt(&app, "info", format!("复用已下载的压缩包: {}", archive_path.display()));
        }
        log_evt(
            &app,
            "info",
            format!(
                "下载 {id}: {}（{}）",
                plan.url,
                if plan.expected_sha256.is_some() {
                    "SHA-256 校验可用"
                } else {
                    "无校验和，跳过完整性校验"
                }
            ),
        );
        let progress = spawn_progress_emitter(app.clone(), task_id.clone());
        let archive = flow::download_with_plan(
            &home,
            &settings,
            &release,
            plan,
            Some(progress),
            Some(Arc::clone(&token)),
        )
        .await?;

        if let Some(dir_name) = dir_name {
            if token.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(jvm_core::Error::Cancelled);
            }
            emit_stage(&app, &task_id, "extract", "");
            // 解压是数百 MB 级阻塞 IO（含 java -version 验证，最长 30s）：
            // 放线程池执行，避免长时间占死 async worker。
            let home2 = home.clone();
            let release2 = release.clone();
            let archive2 = archive.clone();
            let dir_name2 = dir_name.clone();
            tauri::async_runtime::spawn_blocking(move || {
                flow::install_downloaded(&home2, &release2, &archive2, Some(&dir_name2))
            })
            .await
            .map_err(|e| jvm_core::Error::msg(format!("解压任务失败: {e}")))??;
            // 设置开启时清理下载压缩包（一键安装与先下载再安装都走这里）
            if settings.auto_clean_download && archive.exists() {
                match std::fs::remove_file(&archive) {
                    Ok(()) => {
                        log_evt(&app, "info", format!("已清除下载文件: {}", archive.display()))
                    }
                    Err(e) => log_evt(
                        &app,
                        "warn",
                        format!("清除下载文件失败 {}: {e}", archive.display()),
                    ),
                }
            }
        }
        Ok(())
    }
    .await;

    // 仅当登记的仍是本次任务的 token 时才移除：防止误删后启动任务的
    // 保护标记（任务结束后才允许同 id 重新发起）。
    state
        .cancels
        .lock()
        .unwrap()
        .retain(|k, v| !(k == &task_id && Arc::ptr_eq(v, &token)));
    match result {
        Ok(()) => {
            log_evt(&app, "success", format!("{verb}完成: {id}"));
            emit_stage(&app, &task_id, "done", "");
        }
        Err(jvm_core::Error::Cancelled) => {
            log_evt(&app, "warn", format!("{verb}已取消: {id}"));
            emit_stage(&app, &task_id, "cancelled", "已取消");
        }
        Err(e) => {
            log_evt(&app, "error", format!("{verb}失败: {id} — {e}"));
            emit_stage(&app, &task_id, "error", &e.to_string());
        }
    }
}

#[tauri::command]
async fn install_jdk(
    app: AppHandle,
    state: State<'_, SharedState>,
    release: ReleaseDto,
    dir_name: String,
) -> Result<String, String> {
    let st: SharedState = state.inner().clone();
    let task_id = release.id.clone();
    let id = release.id.clone();
    let dto_release = release.to_release();
    let ret = task_id.clone();
    let token = Arc::new(AtomicBool::new(false));
    if !register_task(&st, &task_id, Arc::clone(&token)) {
        return Err(format!("任务已在进行中: {task_id}"));
    }
    tauri::async_runtime::spawn(async move {
        run_install_or_download(app, &st, task_id, id, dto_release, Some(dir_name), token).await;
    });
    Ok(ret)
}

#[tauri::command]
async fn download_jdk(
    app: AppHandle,
    state: State<'_, SharedState>,
    release: ReleaseDto,
) -> Result<String, String> {
    let st: SharedState = state.inner().clone();
    let task_id = release.id.clone();
    let id = release.id.clone();
    let dto_release = release.to_release();
    let ret = task_id.clone();
    let token = Arc::new(AtomicBool::new(false));
    if !register_task(&st, &task_id, Arc::clone(&token)) {
        return Err(format!("任务已在进行中: {task_id}"));
    }
    tauri::async_runtime::spawn(async move {
        run_install_or_download(app, &st, task_id, id, dto_release, None, token).await;
    });
    Ok(ret)
}

/// 已下载到下载目录的包 id 列表：下载目录里已有对应归档（下载与安装
/// 共用同一落盘文件名）即视为已下载，前端把「下载」置灰为「已下载」，
/// 「安装」点击时后端会校验复用该文件，不再重复下载。
#[tauri::command]
fn get_downloaded_ids(releases: Vec<ReleaseDto>) -> Result<Vec<String>, String> {
    let downloads = app_home()?.downloads();
    Ok(releases
        .iter()
        .filter(|r| {
            jvm_core::model::JdkId::parse(&r.id)
                .map(|id| {
                    downloads
                        .join(jvm_core::model::archive_filename(&id, &r.archive_type))
                        .is_file()
                })
                .unwrap_or(false)
        })
        .map(|r| r.id.clone())
        .collect())
}

#[tauri::command]
async fn cancel_task(state: State<'_, SharedState>, task_id: String) -> Result<(), String> {
    if let Some(token) = state.cancels.lock().unwrap().get(&task_id) {
        token.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

// ---------- 项目绑定（我们保留的页面） ----------

#[tauri::command]
async fn list_projects() -> Result<Vec<ProjectEntry>, String> {
    let home = app_home()?;
    Ok(store::load_projects(&home).map_err(estr)?.projects)
}

#[tauri::command]
async fn bind_project(path: String, id: String) -> Result<bool, String> {
    let home = app_home()?;
    let dir = std::path::PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("目录不存在: {path}"));
    }
    let index = store::load_index(&home).map_err(estr)?;
    let rec = index
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("未安装的版本 {id}"))?;
    flow::bind_project(&home, &dir, &rec.jdk_id()).map_err(estr)
}

#[tauri::command]
async fn unbind_project(path: String) -> Result<bool, String> {
    let home = app_home()?;
    flow::unbind_project(&home, std::path::Path::new(&path)).map_err(estr)
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_folder()
            .map(|fp| fp.to_string())
    })
    .await
    .map_err(estr)
}

#[tauri::command]
async fn get_installed_ids() -> Result<Vec<InstalledJdk>, String> {
    let home = app_home()?;
    Ok(store::load_index(&home).map_err(estr)?.installed)
}

// AppState 需要多处共享（spawn 的任务里也要访问），用 Arc 包装管理。
type SharedState = Arc<AppState>;

/// shim 分发入口：本 exe 被复制为 `java`/`javac` 等名字运行时返回
/// Some(退出码)，main 应立即退出而不启动 GUI；正常 GUI 启动返回 None。
pub fn shim_entry() -> Option<i32> {
    let stem = std::env::current_exe()
        .ok()?
        .file_stem()?
        .to_string_lossy()
        .to_lowercase();
    if !shim::is_shim_tool(&stem) {
        return None;
    }
    attach_parent_console();
    let home = Home::configured();
    Some(shim::run_shim(&home, &stem))
}

/// GUI 子系统进程（release 的 windows_subsystem）没有控制台：从终端启动
/// shim 时附加到父进程终端，`java -version` 的输出才能显示出来。
/// 附加失败静默忽略——管道/重定向环境下 std 句柄本就有效。
#[cfg(windows)]
fn attach_parent_console() {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
    }
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

/// 启动时把 bin/ 下的 shim 同步为当前二进制：应用更新或旧版写错 shim
/// （如曾把无分发逻辑的 GUI 程序复制为 java.exe）后，无需重新「启用」即自愈。
/// 失败仅告警，不阻塞 GUI 启动。
fn refresh_shims() {
    let home = Home::configured();
    if home.ensure_dirs().is_err() {
        return;
    }
    if let Err(e) = switcher::write_shims(&home) {
        tracing::warn!("同步命令 shim 失败: {e}");
    }
}

pub fn run() {
    // 文件日志（~/.jvm/logs/jvm.log）先行：shim 同步、下载校验和缺失等
    // tracing 告警都能落盘可查；guard 泄漏给进程生命周期。
    let log_home = Home::configured();
    if let Some(guard) = jvm_core::logging::init(&log_home) {
        std::mem::forget(guard);
    }

    refresh_shims();

    // 清理上次会话遗留的过期下载断点（取消/失败保留的 .part），
    // 启动时不可能有进行中的下载，按 TTL 回收安全。
    let cleanup_home = Home::configured();
    jvm_core::downloader::clean_stale_parts(
        &cleanup_home.downloads(),
        jvm_core::downloader::STALE_PART_TTL,
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // 窗口状态持久化：记住尺寸/位置/最大化，退出自动保存、启动自动恢复
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(SharedState::new(AppState {
            pkg_cache: Mutex::new(HashMap::new()),
            meta_cache: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
        }))
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            debug_log,
            get_platform,
            get_default_paths,
            get_distributions,
            get_major_versions,
            get_all_releases,
            get_local_jdks,
            get_current_java_home,
            use_jdk,
            uninstall_jdk,
            open_dir,
            install_jdk,
            download_jdk,
            get_downloaded_ids,
            cancel_task,
            list_projects,
            bind_project,
            unbind_project,
            pick_folder,
            get_installed_ids,
        ])
        .run(tauri::generate_context!())
        .expect("tauri 应用启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;
    use jvm_core::model::{Release, RemotePackage};

    fn sample_release() -> Release {
        Release {
            vendor: "temurin".into(),
            version: "17.0.12+7".into(),
            feature: 17,
            term: "lts".into(),
            lts: true,
            package: RemotePackage {
                os: "windows".into(),
                arch: "x64".into(),
                archive_type: "zip".into(),
                size: Some(190_817_615),
                filename: "OpenJDK17U-jdk_x64_windows_hotspot_17.0.12_7.zip".into(),
                distribution_version: "17.0.20.1".into(),
                image_type: "jdk".into(),
                direct_url: Some("https://example/x.zip".into()),
                info_uri: Some("https://api.foojay.io/disco/v3.0/ids/c986".into()),
                checksum_sha256: Some("ab".repeat(32)),
                checksum_url: None,
            },
        }
    }

    #[test]
    fn release_dto_roundtrip_preserves_fields() {
        let dto = ReleaseDto::from_release(&sample_release());
        let back = dto.to_release().unwrap();
        assert_eq!(back.id().to_string(), "temurin@17.0.12+7");
        assert_eq!(back.package.filename, dto.filename);
        assert_eq!(back.package.size, Some(190_817_615));
        assert_eq!(back.package.info_uri, dto.info_uri);
        assert_eq!(back.package.image_type, "jdk");
        assert!(back.lts);
        // 直链/校验和按设计留空：下载阶段按 info_uri 直查补全，
        // 不能按版本串重新解析（部分包 version= 过滤命中不到）。
        assert!(back.package.direct_url.is_none());
        assert!(back.package.checksum_sha256.is_none());
    }

    #[test]
    fn release_dto_serializes_camel_case_for_frontend() {
        let dto = ReleaseDto::from_release(&sample_release());
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["id"], "temurin@17.0.12+7");
        assert_eq!(json["distribution"], "temurin");
        assert_eq!(json["javaVersion"], "17.0.12+7");
        assert_eq!(json["distributionVersion"], "17.0.20.1");
        assert_eq!(json["packageType"], "jdk");
        assert_eq!(json["termOfSupport"], "lts");
        assert_eq!(json["directlyDownloadable"], true);
        assert_eq!(json["infoUri"], "https://api.foojay.io/disco/v3.0/ids/c986");
        assert_eq!(json["size"], 190_817_615);
    }

    #[test]
    fn release_dto_without_feature_version_is_not_installable() {
        let mut dto = ReleaseDto::from_release(&sample_release());
        dto.java_version = String::new();
        assert!(dto.to_release().is_none(), "无法提取主版本时不可安装");
    }

    #[test]
    fn shim_entry_ignores_non_tool_binary_names() {
        // 测试进程的文件名不是 java/javac 等 shim 工具名 → 不进入 shim 模式
        assert!(shim_entry().is_none());
    }
}
