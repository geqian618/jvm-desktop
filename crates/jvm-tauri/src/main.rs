// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // busybox 式 shim：本 exe 被复制为 ~/.jvm/bin/java.exe 等之后，按文件名
    // 进入转发模式直接退出；文件名不是工具名时才启动 GUI。
    if let Some(code) = jvm_tauri_lib::shim_entry() {
        std::process::exit(code);
    }
    jvm_tauri_lib::run()
}
