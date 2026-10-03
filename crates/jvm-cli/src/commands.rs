//! jdm 子命令实现（呈现层；业务编排全部委托 `jvm_core::flow`）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Subcommand;
use indicatif::{ProgressBar, ProgressStyle};
use jvm_core::downloader::ProgressFn;
use jvm_core::flow;
use jvm_core::installer;
use jvm_core::model::JdkId;
use jvm_core::source::{from_settings, PkgQuery};
use jvm_core::store::InstalledJdk;
use jvm_core::{project, shim, store, switcher, Home, Result};

use crate::ui;

#[derive(Subcommand)]
pub enum Cmd {
    /// 列出本地已安装版本
    Ls,
    /// 列出远程可用版本（默认各 LTS 主版本的最新 GA）
    LsRemote {
        /// 发行商（foojay api_parameter），默认 temurin
        #[arg(short, long)]
        vendor: Option<String>,
        /// 列出某主版本的全部更新版本（需提供 feature）
        #[arg(short, long)]
        all: bool,
        /// 主版本号，如 17
        feature: Option<u32>,
        /// 列出 JRE 而非 JDK
        #[arg(long)]
        jre: bool,
    },
    /// 显示当前目录解析到的 JDK
    Current,
    /// 切换默认版本；--project 时绑定当前目录（写入 .jvmrc）
    Use {
        /// 版本，如 temurin@17.0.12+7 / temurin@17 / 17
        id: String,
        #[arg(long)]
        project: bool,
    },
    /// 安装版本，如 temurin@17 或 temurin@17.0.12+7
    Install {
        id: String,
        /// 安装后设为默认
        #[arg(long)]
        default: bool,
    },
    /// 卸载版本
    Uninstall { id: String },
    /// 打印某命令解析到的实际路径
    Which { tool: String },
    /// 导入本机已有 JDK 目录
    Import {
        path: PathBuf,
        /// 显式指定标识，如 temurin@17.0.12+7；缺省按目录名推断
        #[arg(long = "as")]
        as_id: Option<String>,
    },
    /// 诊断 PATH / JAVA_HOME / shim 配置
    Doctor,
    /// 打印 jvm 数据目录
    Home,
}

pub async fn run(cmd: Cmd, home: &Home) -> Result<()> {
    home.ensure_dirs()?;
    match cmd {
        Cmd::Ls => ls(home),
        Cmd::LsRemote {
            vendor,
            all,
            feature,
            jre,
        } => ls_remote(home, vendor, all, feature, jre).await,
        Cmd::Current => current(home),
        Cmd::Use { id, project } => use_version(home, &id, project).await,
        Cmd::Install { id, default } => install(home, &id, default).await,
        Cmd::Uninstall { id } => uninstall(home, &id),
        Cmd::Which { tool } => which(home, &tool),
        Cmd::Import { path, as_id } => import(home, &path, as_id),
        Cmd::Doctor => doctor(home),
        Cmd::Home => print_home(home),
    }
}

// ---------- ls ----------

fn ls(home: &Home) -> Result<()> {
    let index = store::load_index(home)?;
    if index.installed.is_empty() {
        println!(
            "还没有已安装的版本，试试 {} 开始。",
            ui::accent("jvm install temurin@17")
        );
        return Ok(());
    }
    let default = store::read_default(home)?;
    let mut records = index.installed.clone();
    records.sort_by(|a, b| {
        a.vendor
            .cmp(&b.vendor)
            .then_with(|| jvm_core::model::cmp_versions(&b.version, &a.version))
    });
    for rec in &records {
        let mark = if default.as_ref().map(|d| d.to_string()) == Some(rec.id.clone()) {
            ui::accent("->")
        } else {
            "  ".to_string()
        };
        let lts = if rec.lts { "LTS" } else { "   " };
        let imported = if rec.imported { "  (导入)" } else { "" };
        // 与 GUI 一致：转本地时区日期（installed_at 是 UTC RFC3339，
        // 直接截取前 10 位在跨日界时区会差一天）
        let date = rec.installed_date().unwrap_or_default();
        println!(
            "{mark} {:<32} {:>2}  {}  {}{}",
            rec.id,
            rec.feature,
            lts,
            ui::dim(&date),
            imported
        );
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match project::resolve(&cwd, home) {
        Some(r) => {
            let where_ = match &r {
                project::Resolution::RcFile { root, .. } => {
                    format!("项目 .jvmrc（{}）", root.display())
                }
                project::Resolution::Registry { root, .. } => {
                    format!("projects.json 登记（{}）", root.display())
                }
                project::Resolution::Global(_) => "全局默认".to_string(),
            };
            println!(
                "\n当前目录生效: {}  {}",
                ui::accent(r.id().to_string()),
                ui::dim(&where_)
            );
        }
        None => println!("\n当前目录未解析到任何版本。"),
    }
    Ok(())
}

// ---------- ls-remote ----------

async fn ls_remote(
    home: &Home,
    vendor: Option<String>,
    all: bool,
    feature: Option<u32>,
    jre: bool,
) -> Result<()> {
    let settings = store::load_settings(home)?;
    let source = from_settings(&settings)?;
    let vendor = vendor.unwrap_or_else(|| "temurin".to_string());
    let image = if jre { "jre" } else { "jdk" };
    println!(
        "数据源: {}   发行商: {}   平台: {}/{} ({})",
        ui::accent(source.spec_name()),
        ui::accent(&vendor),
        jvm_core::model::current_os(),
        jvm_core::model::current_arch(),
        image
    );

    match (all, feature) {
        (true, Some(f)) => {
            let versions = source.version_strings(&vendor, f).await?;
            if versions.is_empty() {
                println!("未找到 {vendor} 的 {f} 版本。");
                return Ok(());
            }
            println!("共 {} 个 GA 版本（首个为最新）：", versions.len());
            for (i, v) in versions.iter().enumerate() {
                let mark = if i == 0 {
                    ui::accent("*")
                } else {
                    " ".to_string()
                };
                println!("{mark} {vendor}@{v}");
            }
        }
        (true, None) => {
            println!("--all 需要提供主版本号，例如: jvm ls-remote --all 17");
        }
        (false, Some(f)) => {
            let query = PkgQuery::for_local_machine(&vendor, f).with_image_type(image);
            if let Some(r) = source.latest_for_feature(&query).await? {
                print_release(&r);
            } else {
                println!("未找到 {vendor} {f} 在当前平台的可用包。");
            }
        }
        (false, None) => {
            let features = source.lts_features().await?;
            if features.is_empty() {
                return Err(jvm_core::Error::msg("数据源未返回任何 LTS 主版本"));
            }
            println!("{:<36} {:<4} 大小", "版本", "LTS");
            for f in &features {
                let query = PkgQuery::for_local_machine(&vendor, *f).with_image_type(image);
                if let Some(r) = source.latest_for_feature(&query).await? {
                    print_release(&r);
                }
            }
            println!(
                "\n查看某主版本全部更新: {}",
                ui::accent(format!(
                    "jvm ls-remote --all {}",
                    features.last().copied().unwrap_or(25)
                ))
            );
        }
    }
    Ok(())
}

fn print_release(r: &jvm_core::model::Release) {
    let lts = if r.lts { "LTS" } else { "   " };
    let size = r
        .package
        .size
        .map(ui::human_size)
        .unwrap_or_else(|| "-".to_string());
    println!("{:<36} {:<4} {}", r.id().to_string(), lts, size);
}

// ---------- current ----------

fn current(home: &Home) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match project::resolve(&cwd, home) {
        Some(r) => {
            let where_ = match &r {
                project::Resolution::RcFile { root, .. } => {
                    format!("项目 .jvmrc（{}）", root.display())
                }
                project::Resolution::Registry { root, .. } => {
                    format!("projects.json 登记（{}）", root.display())
                }
                project::Resolution::Global(_) => "全局默认".to_string(),
            };
            println!("{}  {}", ui::accent(r.id().to_string()), ui::dim(&where_));
            if let Ok((bin, _)) = shim::resolve_tool(home, &cwd, "java") {
                println!("java → {}", ui::dim(bin.display()));
            }
        }
        None => println!("未设置任何 JDK（既无项目关联也无全局默认）。"),
    }
    Ok(())
}

// ---------- use ----------

async fn use_version(home: &Home, id: &str, as_project: bool) -> Result<()> {
    let index = store::load_index(home)?;
    let (vendor, r#ref) = flow::split_id(id);
    let rec = flow::match_installed(&index, vendor, &r#ref)
        .ok_or_else(|| jvm_core::Error::NotFound(id.to_string()))?;
    let jdk_id = rec.jdk_id();

    if as_project {
        let cwd = std::env::current_dir()?;
        flow::bind_project(home, &cwd, &jdk_id)?;
        println!(
            "已把 {} 绑定到当前目录（{}），并生成 .jvmrc（可提交到 Git）",
            ui::accent(jdk_id.to_string()),
            cwd.display()
        );
    } else {
        let outcome = flow::switch_default(home, &jdk_id)?;
        println!("默认 JDK 已切换为 {}", ui::accent(jdk_id.to_string()));
        // 仅在环境变量实际变更（首次配置/修复）时提示重开终端；
        // 日常切换即时生效，无需多余输出。
        for hint in &outcome.hints {
            println!("{}", ui::warn(hint));
        }
    }
    Ok(())
}

// ---------- install ----------

async fn install(home: &Home, id: &str, set_default: bool) -> Result<()> {
    if flow::split_id(id).0.is_none() {
        println!("{}", ui::dim("未指定发行商，默认使用 temurin（可用 temurin/zulu/corretto/graalvm_community/dragonwell/microsoft 等）"));
    }
    let settings = store::load_settings(home)?;
    let release = flow::resolve_remote_release(&settings, id).await?;

    let jdk_id = release.id();
    println!(
        "目标版本: {}（{}）",
        ui::accent(jdk_id.to_string()),
        if release.lts { "LTS" } else { "非 LTS" }
    );
    // 经 /ids/{id} 详情补全真实直链（列表接口大多只有重定向或占位）
    let plan = flow::plan_download_for_release(&settings, &release).await?;
    println!("下载: {}", ui::dim(&plan.url));

    let progress = progress_bar(plan.size);
    let rec = flow::install_release_with_plan(home, &settings, &release, plan, progress).await?;
    for line in rec.java_version_output.lines() {
        println!("  {}", ui::dim(line));
    }
    println!(
        "安装完成 → {}",
        ui::dim(home.version_dir(&jdk_id).display())
    );

    if set_default {
        let outcome = flow::switch_default(home, &jdk_id)?;
        println!("已设为默认版本 {}", ui::accent(jdk_id.to_string()));
        for hint in &outcome.hints {
            println!("{}", ui::warn(hint));
        }
    } else {
        println!("设为默认: {}", ui::accent(format!("jvm use {jdk_id}")));
    }
    Ok(())
}

/// 构造下载进度条；回调来自多个分块任务线程，indicatif 线程安全。
fn progress_bar(total: Option<u64>) -> Option<Arc<ProgressFn>> {
    let pb = match total {
        Some(t) if t > 0 => ProgressBar::new(t),
        _ => ProgressBar::new_spinner(),
    };
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} {eta} {bytes_per_sec}",
    )
    .unwrap()
    .progress_chars("=>-");
    pb.set_style(style);
    let pb = Arc::new(pb);
    let cb_pb = Arc::clone(&pb);
    Some(Arc::new(move |done: u64, total: u64| {
        if total > 0 && cb_pb.length() != Some(total) {
            cb_pb.set_length(total);
        }
        cb_pb.set_position(done);
    }))
}

// ---------- uninstall ----------

fn uninstall(home: &Home, id: &str) -> Result<()> {
    let index = store::load_index(home)?;
    let (vendor, r#ref) = flow::split_id(id);
    let rec = flow::match_installed(&index, vendor, &r#ref)
        .ok_or_else(|| jvm_core::Error::NotFound(id.to_string()))?;
    let jdk_id = rec.jdk_id();

    flow::uninstall(home, &jdk_id)?;
    println!("已卸载 {}", ui::accent(jdk_id.to_string()));
    Ok(())
}

// ---------- which ----------

fn which(home: &Home, tool: &str) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (bin, jdk_home) = shim::resolve_tool(home, &cwd, tool)?;
    println!("{}", bin.display());
    println!("JAVA_HOME={}", ui::dim(jdk_home.display()));
    Ok(())
}

// ---------- import ----------

fn import(home: &Home, path: &Path, as_id: Option<String>) -> Result<()> {
    let id = match as_id {
        Some(s) => JdkId::parse(&s)?,
        None => {
            // 按目录名推断：jdk-17.0.12+7 → local@17.0.12+7
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let version = name.strip_prefix("jdk-").unwrap_or(&name).trim().to_string();
            if version.is_empty() {
                return Err(jvm_core::Error::msg(format!(
                    "无法从目录名 `{name}` 推断版本，请用 --as <vendor@version> 指定"
                )));
            }
            JdkId::new("local", version)
        }
    };
    println!(
        "导入 {} 为 {}",
        ui::dim(path.display()),
        ui::accent(id.to_string())
    );
    let outcome = installer::import_directory(path, &id, home, &installer::run_java_version)?;
    // 与 GUI 安装共用同一把索引写锁，避免并发安装/登记互相覆盖
    store::update_index(home, |index| {
        index.upsert(InstalledJdk {
            id: id.to_string(),
            name: format!("{}-{}", id.vendor, id.version),
            vendor: id.vendor.clone(),
            version: id.version.clone(),
            feature: jvm_core::model::version_feature(&id.version).unwrap_or(0),
            lts: false,
            dir: outcome.dir_name,
            java_version_output: outcome.java_version_output,
            imported: true,
            external: false,
            installed_at: chrono::Utc::now().to_rfc3339(),
        });
        Ok(())
    })?;
    println!("导入完成（标记 imported，卸载只会删除 jvm 内的拷贝）");
    Ok(())
}

// ---------- doctor ----------

fn doctor(home: &Home) -> Result<()> {
    let mut failures = 0usize;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match home.ensure_dirs() {
        Ok(()) => println!("{}", ui::ok(format!("数据目录 {}", home.root().display()))),
        Err(e) => {
            failures += 1;
            println!("{}", ui::fail(format!("数据目录不可写: {e}")));
        }
    }

    let index = store::load_index(home)?;
    println!(
        "{}",
        ui::ok(format!("已安装版本数: {}", index.installed.len()))
    );
    let default = store::read_default(home)?;
    let expected_target = match &default {
        Some(id) => match index.get(&id.to_string()) {
            Some(rec) => {
                let dir = store::resolve_dir(home, &rec.dir);
                match installer::jdk_home_of(&dir) {
                    Some(_) => {
                        println!(
                            "{}",
                            ui::ok(format!("默认版本 {id} → {}", dir.display()))
                        );
                        Some(dir)
                    }
                    None => {
                        failures += 1;
                        println!(
                            "{}",
                            ui::fail(format!(
                                "默认版本 {id} 的目录损坏（缺 bin/java）: {}",
                                dir.display()
                            ))
                        );
                        None
                    }
                }
            }
            None => {
                failures += 1;
                println!(
                    "{}",
                    ui::fail(format!("默认版本 {id} 不在索引中，重新安装或切换"))
                );
                None
            }
        },
        None => {
            println!("{}", ui::warn("未设置默认版本（jvm use <版本>）"));
            None
        }
    };

    let current = home.current();
    if current.symlink_metadata().is_ok() {
        if expected_target.is_some() {
            println!(
                "{}",
                ui::ok(format!("current 链接存在: {}", current.display()))
            );
        }
    } else if default.is_some() {
        failures += 1;
        println!("{}", ui::fail("current 链接缺失，重新执行 jvm use 修复"));
    }

    let exe = std::env::current_exe().ok();
    let exe_size = exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    // 仅当当前进程就是名为 jvm 的 CLI 时才比较 shim 新鲜度（开发期常见
    // 场景：target/ 下的新二进制 vs bin/ 里的旧拷贝）。
    let exe_is_cli = exe.as_deref().map(switcher::is_cli_exe).unwrap_or(false);
    for cmd in switcher::managed_commands() {
        let cmd_path = home.bin().join(installer::exe_name(cmd));
        if !cmd_path.exists() {
            println!(
                "{}",
                ui::warn(format!("命令 {} 缺失，执行 jvm use 重建", cmd_path.display()))
            );
        } else if exe_is_cli {
            if let Some(sz) = exe_size {
                let cmd_size = std::fs::metadata(&cmd_path).map(|m| m.len()).unwrap_or(0);
                if cmd_size != sz {
                    println!(
                        "{}",
                        ui::warn(format!(
                            "{cmd} 与当前可执行文件不一致（过旧），建议 jvm use 重建"
                        ))
                    );
                }
            }
        }
    }
    println!("{}", ui::ok(format!("shim 目录: {}", home.bin().display())));

    check_path(home, &mut failures);
    check_java_home(home, &mut failures);
    check_shadowing(home);

    if default.is_some() {
        let java_shim = home.bin().join(installer::exe_name("java"));
        if java_shim.exists() {
            match std::process::Command::new(&java_shim).arg("-version").output() {
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stderr).to_string()
                        + &String::from_utf8_lossy(&out.stdout);
                    println!(
                        "{} {}",
                        ui::ok("shim java -version:"),
                        ui::dim(text.trim())
                    );
                }
                Err(e) => {
                    failures += 1;
                    println!("{}", ui::fail(format!("运行 shim java 失败: {e}")));
                }
            }
        }
    }

    match project::resolve(&cwd, home) {
        Some(r) => println!(
            "当前目录（{}）生效: {}",
            cwd.display(),
            ui::accent(r.id().to_string())
        ),
        None => println!("当前目录无绑定。"),
    }

    if failures > 0 {
        Err(jvm_core::Error::msg(format!(
            "doctor 发现 {failures} 项问题"
        )))
    } else {
        println!("{}", ui::ok("全部检查通过"));
        Ok(())
    }
}

fn check_path(home: &Home, failures: &mut usize) {
    let bin = home.bin().to_string_lossy().to_string();
    let in_process = std::env::var("PATH")
        .map(|p| {
            p.split(';')
                .any(|e| e.replace('/', "\\").eq_ignore_ascii_case(&bin))
        })
        .unwrap_or(false);
    #[cfg(windows)]
    let in_registry = jvm_core::env::windows::user_path_contains(&home.bin());
    #[cfg(not(windows))]
    let in_registry = in_process;

    if in_registry {
        println!(
            "{}",
            ui::ok(format!(
                "用户 PATH 已包含 shim 目录{}",
                if in_process {
                    ""
                } else {
                    "（当前终端未生效，重开终端）"
                }
            ))
        );
    } else {
        *failures += 1;
        println!(
            "{}",
            ui::fail(format!(
                "用户 PATH 缺少 {}，执行 jvm use 自动补上",
                home.bin().display()
            ))
        );
    }
}

fn check_java_home(home: &Home, failures: &mut usize) {
    let want = home.current().to_string_lossy().to_string();
    #[cfg(windows)]
    {
        match jvm_core::env::windows::get_user_env("JAVA_HOME") {
            Some(v) if v.eq_ignore_ascii_case(&want) => {
                println!("{}", ui::ok(format!("注册表 JAVA_HOME = {v}")));
            }
            Some(v) => {
                *failures += 1;
                println!(
                    "{}",
                    ui::fail(format!(
                        "注册表 JAVA_HOME = {v}，应为 {want}；执行 jvm use 修复"
                    ))
                );
            }
            None => {
                *failures += 1;
                println!(
                    "{}",
                    ui::fail("注册表未设置用户 JAVA_HOME，执行 jvm use 修复")
                );
            }
        }
    }
    match std::env::var("JAVA_HOME") {
        Ok(v) if v.eq_ignore_ascii_case(&want) => {
            println!("{}", ui::ok("当前终端 JAVA_HOME 指向 current"));
        }
        Ok(v) => println!(
            "{}",
            ui::warn(format!(
                "当前终端 JAVA_HOME={v}（非 jvm 的 current；新终端生效或手动修正）"
            ))
        ),
        Err(_) => println!(
            "{}",
            ui::warn("当前终端未设置 JAVA_HOME（Windows 重开终端后生效）")
        ),
    }
}

fn check_shadowing(home: &Home) {
    #[cfg(windows)]
    let output = std::process::Command::new("where").arg("java").output();
    #[cfg(not(windows))]
    let output = std::process::Command::new("which").arg("-a").arg("java").output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(first) = text.lines().next() {
            let first = first.trim();
            let ours = home.bin().join(installer::exe_name("java"));
            if !first.eq_ignore_ascii_case(&ours.to_string_lossy()) {
                println!(
                    "{}",
                    ui::warn(format!(
                        "PATH 上第一个 java 是 {first}（遮蔽了 jvm shim {}）",
                        ours.display()
                    ))
                );
                return;
            }
            println!("{}", ui::ok("PATH 首个 java 即 jvm shim"));
        }
    }
}

fn print_home(home: &Home) -> Result<()> {
    println!("root:       {}", home.root().display());
    println!("versions:   {}", home.versions().display());
    println!("bin(shims): {}", home.bin().display());
    println!("current:    {}", home.current().display());
    println!("downloads:  {}", home.downloads().display());
    println!("logs:       {}", home.logs().display());
    println!("index:      {}", home.index_file().display());
    println!("projects:   {}", home.projects_file().display());
    println!("settings:   {}", home.settings_file().display());
    Ok(())
}
