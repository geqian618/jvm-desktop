//! 文件日志：滚动写入 `~/.jvm/logs/`。
//!
//! 只在 CLI/命令型操作中初始化；shim 模式不初始化（每次 `java` 调用
//! 都会经过，零开销优先）。

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::home::Home;

/// 初始化文件日志；返回的 guard 必须存活到进程结束（drop 即停写）。
pub fn init(home: &Home) -> Option<WorkerGuard> {
    if std::fs::create_dir_all(home.logs()).is_err() {
        return None;
    }
    let appender = tracing_appender::rolling::daily(home.logs(), "jvm.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,jvm=info"));
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(writer)
        .try_init()
        .ok();
    Some(guard)
}
