//! jvm 子命令实现（呈现层；业务编排全部委托 `jvm_core::flow`）。

use std::path::PathBuf;
use std::sync::Arc;

use clap::{CommandFactory, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use jvm_core::downloader::ProgressFn;
use jvm_core::flow;
use jvm_core::installer;
use jvm_core::source::{from_settings, PkgQuery, FOOJAY_BASE};
use jvm_core::store::{RegistryPreset, Settings};
use jvm_core::{project, shim, store, switcher, Home, Result};

use crate::ui;

#[derive(Subcommand)]
#[command(disable_help_flag = true)]
pub enum Cmd {
    /// 列出本地已安装版本
    List,
    /// 列出远程可用版本（默认各 LTS 主版本的最新 GA）
    ListRemote {
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
        /// 绑定当前目录（写入 .jvmrc）而非全局默认
        #[arg(long)]
        project: bool,
    },
    /// 安装版本，如 temurin@17 或 temurin@17.0.12+7
    Install {
        /// 版本，如 temurin@17 / temurin@17.0.12+7
        id: String,
        /// 安装后设为默认
        #[arg(long)]
        default: bool,
    },
    /// 卸载版本
    Uninstall {
        /// 要卸载的版本，如 temurin@17.0.12+7 或 17
        id: String,
    },
    /// 打印某命令解析到的实际路径
    Which {
        /// 工具名，如 java / javac / jar
        tool: String,
    },
    /// 诊断 PATH / JAVA_HOME / shim 配置
    Doctor,
    /// 打印 jvm 数据目录
    Home,
    /// 查看与切换数据源（foojay Disco 兼容接口）
    Config {
        #[command(subcommand)]
        cmd: Option<ConfigCmd>,
    },
    /// 打印帮助信息或某子命令的帮助
    Help {
        /// 要查看帮助的子命令，如 install；缺省打印总帮助
        cmd: Option<String>,
    },
}

pub async fn run(cmd: Cmd, home: &Home) -> Result<()> {
    home.ensure_dirs()?;
    match cmd {
        Cmd::List => list(home),
        Cmd::ListRemote {
            vendor,
            all,
            feature,
            jre,
        } => list_remote(home, vendor, all, feature, jre).await,
        Cmd::Current => current(home),
        Cmd::Use { id, project } => use_version(home, &id, project).await,
        Cmd::Install { id, default } => install(home, &id, default).await,
        Cmd::Uninstall { id } => uninstall(home, &id),
        Cmd::Which { tool } => which(home, &tool),
        Cmd::Doctor => doctor(home),
        Cmd::Home => print_home(home),
        Cmd::Config { cmd } => config(home, cmd).await,
        Cmd::Help { cmd } => print_help(cmd.as_deref()),
    }
}

/// `jvm config` 的子命令；缺省（裸 `jvm config`）等同 `list`。
/// config 专用于数据源，语法两级：`jvm config <操作> [参数]`。
/// 预设的增删改语义与 GUI 设置页一致：自定义可删，内置只可编辑。
#[derive(Subcommand)]
#[command(disable_help_flag = true)]
pub(crate) enum ConfigCmd {
    /// 列出当前数据源与预设
    List,
    /// 注册数据源到预设（默认只入列不启用；--default 同时设为当前源）
    Registry {
        /// 数据源 URL（http/https，foojay Disco 兼容接口）
        #[arg(long)]
        url: String,
        /// 数据源名称，如 内部镜像
        #[arg(long)]
        name: String,
        /// 注册后同时设为当前数据源
        #[arg(long)]
        default: bool,
        /// 跳过可达性校验直接保存
        #[arg(long)]
        force: bool,
    },
    /// 切换数据源（按预设名称或 URL）
    Use {
        /// 预设名称或数据源 URL
        target: String,
        /// 跳过可达性校验
        #[arg(long)]
        force: bool,
    },
    /// 编辑预设的名称或 URL（内置预设也可编辑）
    Edit {
        /// 要编辑的预设的现有名称
        name: String,
        /// 新名称；缺省保持不变
        #[arg(long = "name")]
        new_name: Option<String>,
        /// 新 URL；缺省保持不变
        #[arg(long = "url")]
        new_url: Option<String>,
        /// URL 变更时跳过可达性校验
        #[arg(long)]
        force: bool,
    },
    /// 删除自定义数据源预设（内置预设不可删除）
    Remove {
        /// 要删除的预设的名称
        name: String,
    },
}

// ---------- list ----------

fn list(home: &Home) -> Result<()> {
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
        // 与 GUI 一致：转本地时区日期（installed_at 是 UTC RFC3339，
        // 直接截取前 10 位在跨日界时区会差一天）
        let date = rec.installed_date().unwrap_or_default();
        println!(
            "{mark} {:<32} {:>2}  {}  {}",
            rec.id,
            rec.feature,
            lts,
            ui::dim(&date)
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

// ---------- list-remote ----------

async fn list_remote(
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
            println!("--all 需要提供主版本号，例如: jvm list-remote --all 17");
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
                    "jvm list-remote --all {}",
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
                        println!("{}", ui::ok(format!("默认版本 {id} → {}", dir.display())));
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
                ui::warn(format!(
                    "命令 {} 缺失，执行 jvm use 重建",
                    cmd_path.display()
                ))
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
            match std::process::Command::new(&java_shim)
                .arg("-version")
                .output()
            {
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stderr).to_string()
                        + &String::from_utf8_lossy(&out.stdout);
                    println!("{} {}", ui::ok("shim java -version:"), ui::dim(text.trim()));
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
    // Windows 的 PATH 以 ';' 分隔，Unix 以 ':' 分隔；条目与目标统一把
    // '\' 归一为 '/' 后比较（此前 Unix 下按 ';' 切分 + '/'→'\' 替换，
    // 恒不匹配，doctor 永远误报「用户 PATH 缺少」）。
    let bin = home.bin().to_string_lossy().replace('\\', "/");
    let sep = if cfg!(windows) { ';' } else { ':' };
    let in_process = std::env::var("PATH")
        .map(|p| {
            p.split(sep)
                .any(|e| e.replace('\\', "/").eq_ignore_ascii_case(&bin))
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
    let output = std::process::Command::new("which")
        .arg("-a")
        .arg("java")
        .output();

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

// ---------- config ----------

/// 数据源专用指令：裸命令列出当前源与预设，操作直接跟在 config 后
/// （两级语法）。切换/注册前用一次真实的元信息请求校验可达性——存进
/// 一个打不通的源会让后续 list-remote/install 全部失败；校验失败不落盘
/// （--force 跳过校验）。
async fn config(home: &Home, cmd: Option<ConfigCmd>) -> Result<()> {
    let mut settings = store::load_settings(home)?;
    match cmd {
        None | Some(ConfigCmd::List) => registry_list(&settings),
        Some(ConfigCmd::Registry {
            url,
            name,
            default,
            force,
        }) => registry_add(&mut settings, home, url, name, default, force).await,
        Some(ConfigCmd::Use { target, force }) => {
            registry_use(&mut settings, home, target, force).await
        }
        Some(ConfigCmd::Edit {
            name,
            new_name,
            new_url,
            force,
        }) => registry_edit(&mut settings, home, name, new_name, new_url, force).await,
        Some(ConfigCmd::Remove { name }) => registry_remove(&mut settings, home, name),
    }
}

/// 列出当前数据源与预设。API 规范靠 URL 匹配判定，不联网。
fn registry_list(settings: &Settings) -> Result<()> {
    let current = settings.registry.trim().to_string();
    let effective = if current.is_empty() {
        format!("{FOOJAY_BASE}（官方默认）")
    } else {
        current.clone()
    };
    let spec = from_settings(settings)?.spec_name().to_string();
    println!("当前数据源: {}", ui::accent(&effective));
    println!("API 规范:   {}", ui::dim(&spec));
    if !settings.registry_presets.is_empty() {
        println!("\n预设:");
        for p in &settings.registry_presets {
            let mark = if same_source(&p.url, &current) {
                ui::accent("->")
            } else {
                "  ".to_string()
            };
            let tag = if p.built_in {
                ui::dim("（内置）")
            } else {
                String::new()
            };
            println!("{mark} {:<26} {}{}", p.name, ui::dim(&p.url), tag);
        }
        println!(
            "\n切换: {}\n注册: {}\n编辑: {}\n删除: {}",
            ui::accent("jvm config use <名称|URL>"),
            ui::accent("jvm config registry --url <URL> --name <名称> [--default]"),
            ui::accent("jvm config edit <名称> [--name 新名] [--url 新URL]"),
            ui::accent("jvm config remove <名称>")
        );
    }
    Ok(())
}

/// 注册数据源（对齐 GUI「添加数据源」：起名 → 校验 → 入列预设）。
/// 默认只入列不启用；`set_default` 时同时切换当前源。
/// URL 不得与现有预设重复；自定义预设可被 remove 删除。
async fn registry_add(
    settings: &mut Settings,
    home: &Home,
    url: String,
    name: String,
    set_default: bool,
    force: bool,
) -> Result<()> {
    let name = name.trim();
    let url = url.trim().trim_end_matches('/');
    if name.is_empty() || url.is_empty() {
        return Err(jvm_core::Error::msg("数据源需要名称与 URL"));
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(jvm_core::Error::msg(format!(
            "URL 需以 http:// 或 https:// 开头: {url}"
        )));
    }
    if let Some(p) = settings
        .registry_presets
        .iter()
        .find(|p| same_source(&p.url, url))
    {
        return Err(jvm_core::Error::msg(format!(
            "该 URL 已在预设中: {}（{}）",
            p.name, p.url
        )));
    }

    // 可达性校验：临时把新 URL 填进注册表字段构建源，失败不落盘。
    let saved_registry = std::mem::replace(&mut settings.registry, url.to_string());
    let source = from_settings(settings)?;
    if !force {
        println!("校验数据源（{}）…", ui::dim(source.spec_name()));
        match source.distributions().await {
            Ok(list) => println!("可达，{} 个发行版元信息", list.len()),
            Err(e) => {
                settings.registry = saved_registry;
                return Err(jvm_core::Error::msg(format!(
                    "数据源不可达: {e}；确认 URL 与网络后重试，或加 --force 跳过校验"
                )));
            }
        }
    }
    // 默认只入列预设不动当前源；--default 时保留上面换上的新 URL
    if !set_default {
        settings.registry = saved_registry;
    }
    settings.registry_presets.push(RegistryPreset {
        name: name.to_string(),
        url: url.to_string(),
        built_in: false,
    });
    store::save_settings(home, settings)?;
    if set_default {
        println!(
            "已注册数据源并设为当前: {}（{}，API 规范 {}）",
            ui::accent(name),
            ui::accent(url),
            ui::dim(source.spec_name())
        );
    } else {
        println!(
            "已注册数据源: {}（{}，API 规范 {}）；启用: {}",
            ui::accent(name),
            ui::accent(url),
            ui::dim(source.spec_name()),
            ui::accent(format!("jvm config use {name}"))
        );
    }
    Ok(())
}

/// 切换数据源。参数先按预设名称解析（GUI 里的预设由此真正可用），
/// 未命中再按 URL 处理；目标与当前一致时直接短路。
async fn registry_use(
    settings: &mut Settings,
    home: &Home,
    target: String,
    force: bool,
) -> Result<()> {
    let t = target.trim().to_string();
    let preset = settings
        .registry_presets
        .iter()
        .find(|p| preset_matches(p, &t));
    let (url, via_preset) = match preset {
        Some(p) => (p.url.trim().to_string(), Some(p.name.clone())),
        None => {
            if t.starts_with("http://") || t.starts_with("https://") {
                (t, None)
            } else {
                return Err(jvm_core::Error::msg(format!(
                    "既不是预设名称也不是有效的 URL: {t}（jvm config 查看预设列表）"
                )));
            }
        }
    };
    if same_source(&url, settings.registry.trim()) {
        println!(
            "已是当前数据源: {}",
            ui::accent(if url.is_empty() { FOOJAY_BASE } else { &url })
        );
        return Ok(());
    }

    // 校验可达后才写回；失败即返回，磁盘上的设置不受影响。
    let saved_registry = std::mem::replace(&mut settings.registry, url.clone());
    let source = from_settings(settings)?;
    if !force {
        println!("校验数据源（{}）…", ui::dim(source.spec_name()));
        match source.distributions().await {
            Ok(list) => println!("可达，{} 个发行版元信息", list.len()),
            Err(e) => {
                settings.registry = saved_registry;
                return Err(jvm_core::Error::msg(format!(
                    "数据源不可达: {e}；确认 URL 与网络后重试，或加 --force 跳过校验"
                )));
            }
        }
    }
    store::save_settings(home, settings)?;
    let shown = if url.is_empty() {
        format!("{FOOJAY_BASE}（官方默认）")
    } else {
        url
    };
    match via_preset {
        Some(n) => println!(
            "数据源已切换: {}（预设 {}，API 规范 {}）",
            ui::accent(shown),
            ui::accent(n),
            ui::dim(source.spec_name())
        ),
        None => println!(
            "数据源已切换: {}（API 规范 {}）",
            ui::accent(shown),
            ui::dim(source.spec_name())
        ),
    }
    Ok(())
}

/// 预设名称匹配：精确或 ASCII 大小写不敏感。
fn preset_matches(p: &RegistryPreset, t: &str) -> bool {
    let n = p.name.trim();
    n == t || n.eq_ignore_ascii_case(t)
}

/// 内置预设对应的出厂定义：按当前名称（忽略大小写）或 URL 匹配；
/// 两者都已改过时返回 None（此时留空无法确定回退目标，保持报错）。
fn builtin_default_for(p: &RegistryPreset) -> Option<RegistryPreset> {
    store::builtin_presets().into_iter().find(|b| {
        b.name.eq_ignore_ascii_case(p.name.trim())
            || b.url.trim_end_matches('/') == p.url.trim().trim_end_matches('/')
    })
}

/// 编辑预设的名称或 URL（对齐 GUI「编辑数据源」：内置预设也可编辑）。
/// 内置预设字段留空回退出厂默认；URL 变更时校验可达性（--force 跳过）；
/// 被编辑的是当前源时，当前源跟着指向新 URL。
async fn registry_edit(
    settings: &mut Settings,
    home: &Home,
    name: String,
    new_name: Option<String>,
    new_url: Option<String>,
    force: bool,
) -> Result<()> {
    let t = name.trim();
    let idx = settings
        .registry_presets
        .iter()
        .position(|p| preset_matches(p, t))
        .ok_or_else(|| {
            jvm_core::Error::msg(format!("未找到预设: {t}（jvm config 查看预设列表）"))
        })?;
    let old_name = settings.registry_presets[idx].name.clone();
    let old_url = settings.registry_presets[idx].url.clone();
    let new_name = new_name
        .map(|n| n.trim().to_string())
        .unwrap_or_else(|| old_name.clone());
    // URL 归一化去尾斜杠（与 GUI 编辑保存行为一致）
    let new_url = new_url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .unwrap_or_else(|| old_url.clone());
    let mut new_name = new_name;
    let mut new_url = new_url;
    if new_name.is_empty() || new_url.is_empty() {
        // 内置预设字段留空 → 回退出厂默认；自定义预设与身份不明
        // （名称和 URL 都已改过）的内置预设仍视为非法输入。
        match settings.registry_presets[idx]
            .built_in
            .then(|| builtin_default_for(&settings.registry_presets[idx]))
            .flatten()
        {
            Some(f) => {
                if new_name.is_empty() {
                    new_name = f.name;
                }
                if new_url.is_empty() {
                    new_url = f.url;
                }
            }
            None => return Err(jvm_core::Error::msg("名称与 URL 不能为空")),
        }
    }
    if !new_url.starts_with("http://") && !new_url.starts_with("https://") {
        return Err(jvm_core::Error::msg(format!(
            "URL 需以 http:// 或 https:// 开头: {new_url}"
        )));
    }
    if let Some(p) = settings
        .registry_presets
        .iter()
        .enumerate()
        .find(|(i, p)| *i != idx && same_source(&p.url, &new_url))
        .map(|(_, p)| p)
    {
        return Err(jvm_core::Error::msg(format!(
            "该 URL 已在预设中: {}（{}）",
            p.name, p.url
        )));
    }

    let url_changed = !same_source(&old_url, &new_url);
    let was_current = same_source(&old_url, settings.registry.trim());
    // URL 变更时先临时换上用于校验；非当前源的预设编辑完成后要还原。
    let saved_registry = std::mem::replace(&mut settings.registry, new_url.clone());
    let source = from_settings(settings)?;
    if url_changed && !force {
        println!("校验数据源（{}）…", ui::dim(source.spec_name()));
        match source.distributions().await {
            Ok(list) => println!("可达，{} 个发行版元信息", list.len()),
            Err(e) => {
                settings.registry = saved_registry;
                return Err(jvm_core::Error::msg(format!(
                    "数据源不可达: {e}；确认 URL 与网络后重试，或加 --force 跳过校验"
                )));
            }
        }
    }
    if !was_current {
        settings.registry = saved_registry;
    }
    {
        let p = &mut settings.registry_presets[idx];
        p.name = new_name.clone();
        p.url = new_url.clone();
    }
    store::save_settings(home, settings)?;
    println!(
        "已编辑预设: {} → {}（{}）{}",
        ui::dim(&old_name),
        ui::accent(&new_name),
        ui::accent(&new_url),
        if was_current {
            ui::dim("，为当前数据源")
        } else {
            String::new()
        }
    );
    Ok(())
}

/// 删除自定义预设；当前源被删时回退官方默认（与 GUI 行为一致）。
fn registry_remove(settings: &mut Settings, home: &Home, name: String) -> Result<()> {
    let t = name.trim();
    let idx = settings
        .registry_presets
        .iter()
        .position(|p| preset_matches(p, t))
        .ok_or_else(|| {
            jvm_core::Error::msg(format!("未找到预设: {t}（jvm config 查看预设列表）"))
        })?;
    if settings.registry_presets[idx].built_in {
        return Err(jvm_core::Error::msg(format!(
            "内置预设不可删除: {}（可在 GUI 设置页编辑名称/地址）",
            settings.registry_presets[idx].name
        )));
    }
    let removed = settings.registry_presets.remove(idx);
    let mut note = String::new();
    if same_source(&removed.url, settings.registry.trim()) {
        settings.registry = String::new();
        note = "；它是当前数据源，已回退官方默认".to_string();
    }
    store::save_settings(home, settings)?;
    println!(
        "已删除预设: {}（{}）{}",
        ui::accent(&removed.name),
        ui::dim(&removed.url),
        note
    );
    Ok(())
}

/// 两个数据源 URL 是否指向同一处：去空白与尾斜杠后比较；
/// 空串与官方 Foojay 地址等价（settings 中空 = 官方默认）。
fn same_source(a: &str, b: &str) -> bool {
    let norm = |u: &str| u.trim().trim_end_matches('/').to_string();
    let (a, b) = (norm(a), norm(b));
    if a == b {
        return true;
    }
    let official = FOOJAY_BASE.trim_end_matches('/');
    (a.is_empty() && b == official) || (b.is_empty() && a == official)
}

// ---------- help ----------

/// 打印帮助：接管 clap 内建 help 子命令的职责（内建版引导文案是英文，
/// 与全中文的命令描述不一致）。从同一份 Command 树渲染，保证
/// `jvm help` / `jvm help <子命令>` 与 `--help` 输出的语言、命名
/// （jvm 而非 jvm.exe）与版式一致。
fn print_help(target: Option<&str>) -> Result<()> {
    let mut root = crate::Cli::command();
    root.build();
    match target {
        None => Ok(root.print_help()?),
        Some(name) => {
            let sub = root.find_subcommand_mut(name).ok_or_else(|| {
                jvm_core::Error::msg(format!("未知命令: {name}（jvm --help 查看全部命令）"))
            })?;
            Ok(sub.print_help()?)
        }
    }
}
