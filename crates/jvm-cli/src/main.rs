//! jvm 命令行入口。
//!
//! busybox 式自分发：同一个二进制按文件名决定角色——
//! 名为 `jvm` 时是 CLI；被复制为 `java`/`javac`/… 时进入 shim 模式，
//! 这样 `~/.jvm/bin` 下的一组命令无需携带多个可执行文件。

mod commands;
mod ui;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "jvm",
    version,
    about = "JDK 版本管理工具（nvm-desktop 同源设计，GUI + CLI 双模式）",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: commands::Cmd,
}

#[tokio::main]
async fn main() {
    // shim 模式：文件名（argv[0] 去后缀）是高频工具名时直接转发，
    // 不解析参数、不初始化日志。
    if let Some(stem) = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
    {
        if jvm_core::shim::is_shim_tool(&stem) {
            let home = jvm_core::Home::configured();
            std::process::exit(jvm_core::shim::run_shim(&home, &stem));
        }
    }

    let cli = Cli::parse();
    let home = jvm_core::Home::configured();
    // guard 生命周期覆盖整个进程，drop 即停止日志线程。
    let _log_guard = jvm_core::logging::init(&home);

    // 顺手回收过期下载断点（取消/失败遗留的 .part，TTL 内保留供续传）
    jvm_core::downloader::clean_stale_parts(&home.downloads(), jvm_core::downloader::STALE_PART_TTL);

    if let Err(e) = commands::run(cli.cmd, &home).await {
        eprintln!("{} {e}", ui::fail("error:"));
        std::process::exit(1);
    }
}
