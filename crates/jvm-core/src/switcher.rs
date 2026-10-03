//! 切换器：重建 current 链接 → 重建 shim → 写 default → 更新环境变量。
//! （先建链后写默认：任一步骤失败都不破坏 default 与链接的一致性。）

use std::path::PathBuf;

use crate::env::UserEnvWriter;
use crate::error::{Error, Result};
use crate::home::Home;
use crate::installer;
use crate::model::JdkId;
use crate::store::{self, InstalledJdk};

/// shim 覆盖的高频命令；其余工具靠 JAVA_HOME（mvn/gradle 等自读）。
pub const SHIM_TOOLS: [&str; 7] = ["java", "javac", "jar", "jshell", "jlink", "jpackage", "javaw"];

/// CLI 自身进驻 `bin/` 时使用的文件名（与工具 shim 同目录）。
pub const CLI_COMMAND: &str = "jvm";

/// `bin/` 下由本工具管理的全部命令名：7 个工具 shim + CLI 自身。
pub fn managed_commands() -> impl Iterator<Item = &'static str> {
    SHIM_TOOLS.into_iter().chain(std::iter::once(CLI_COMMAND))
}

/// 该 exe 是否为 CLI 本体（文件名为 jvm）。GUI 的二进制即使被复制为
/// `bin/jvm` 也只会按文件名分发成 GUI，所以只允许 CLI 本体写它。
pub fn is_cli_exe(exe: &std::path::Path) -> bool {
    exe.file_stem()
        .map(|s| s.eq_ignore_ascii_case(CLI_COMMAND))
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct SwitchOutcome {
    pub id: JdkId,
    /// current 链接的目标（即所选 JDK 的 home，macOS 下为 Contents/Home）。
    pub jdk_home: PathBuf,
    /// 本次是否实际变更了用户环境变量（首次配置/被外部改动后修复为 true；
    /// 日常切换时值不变为 false，已开终端即时生效无需重开）。
    pub env_updated: bool,
    /// 需要展示给用户的平台相关提示。
    pub hints: Vec<String>,
}

/// 切换全局默认版本。
pub fn switch_to(home: &Home, id: &JdkId, env: &mut dyn UserEnvWriter) -> Result<SwitchOutcome> {
    let index = store::load_index(home)?;
    let rec: InstalledJdk = index
        .get(&id.to_string())
        .cloned()
        .ok_or_else(|| Error::NotFound(id.to_string()))?;
    let dir = store::resolve_dir(home, &rec.dir);
    if !dir.is_dir() {
        return Err(Error::IndexOutOfSync(dir.display().to_string()));
    }
    let jdk_home =
        installer::jdk_home_of(&dir).ok_or_else(|| Error::IndexOutOfSync(dir.display().to_string()))?;

    // 顺序：先重建 current 链接与 shim，最后才写 default——链接重建失败
    // （如残留非 junction 条目、目标被占用）时 default 仍指向旧版本，状态
    // 保持一致、重试即可；若先写 default，中途失败会让 default 与 current
    // 链接指向不同版本，shim 与 JAVA_HOME 各用一头。
    rebuild_current_link(home, &jdk_home)?;
    write_shims(home)?;
    store::write_default(home, id)?;

    let mut outcome = SwitchOutcome {
        id: id.clone(),
        jdk_home,
        env_updated: false,
        hints: Vec::new(),
    };

    // JAVA_HOME 固定指向 current 链接：之后切换只重建链接，无需再改注册表。
    // 平台差异：Windows 直接写用户环境变量；Unix 按提示词要求只提示用户
    // 自行在 shell rc 中加入（不自动改 rc 文件）。
    #[cfg(windows)]
    {
        // 先探测再写入：只有注册表值真的变化（首次配置或被外部改动后修复）
        // 才提示重开终端。日常切换时 JAVA_HOME 恒为 current 链接、PATH 恒含
        // bin，注册表不变——已开终端里 java（shim 每次动态解析 default）与
        // mvn/gradle（JAVA_HOME → current 链接，目标已换新）都即时生效。
        let current_str = home.current().to_string_lossy().to_string();
        let java_home_changed = crate::env::windows::get_user_env("JAVA_HOME")
            .map(|v| !v.eq_ignore_ascii_case(&current_str))
            .unwrap_or(true);
        let path_changed = !crate::env::windows::user_path_contains(&home.bin());
        env.set("JAVA_HOME", &current_str)?;
        env.append_path_entry(&home.bin())?;
        outcome.env_updated = java_home_changed || path_changed;
        if outcome.env_updated {
            outcome.hints.push(
                "已写入用户环境变量 JAVA_HOME/PATH，已开着的终端请重启后生效；日常切换无需重启，java 等命令即时生效"
                    .to_string(),
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = env;
        outcome.hints.push(
            "请将以下两行加入 ~/.bashrc 或 ~/.zshrc 后重开终端：\n  export JAVA_HOME=\"$HOME/.jvm/current\"\n  export PATH=\"$HOME/.jvm/bin:$PATH\""
                .to_string(),
        );
    }

    Ok(outcome)
}

/// 重建 `current` 链接。
///
/// Windows 用 junction（目录联接）而非符号链接：symlink 需要管理员权限
/// 或开发者模式，junction 普通用户即可创建。
pub fn rebuild_current_link(home: &Home, target: &std::path::Path) -> Result<()> {
    let link = home.current();
    #[cfg(windows)]
    {
        // 用 symlink_metadata 判断联接类型而非 junction::exists：后者对
        // 「目标已消失的悬空联接」（如数据目录整体改名后的 current）返回
        // false，会让重建被误拒。std 对 junction 的 symlink_metadata 报告
        // is_symlink == true，悬空时依然成立。
        if let Ok(meta) = link.symlink_metadata() {
            if !meta.file_type().is_symlink() {
                return Err(Error::msg(format!(
                    "{} 已存在但不是 junction，请手动处理后重试",
                    link.display()
                )));
            }
            // remove_dir 只删除联接本身，不会递归删除目标内容。
            std::fs::remove_dir(&link)?;
        }
        junction::create(target, &link)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let meta = link.symlink_metadata();
        if let Ok(m) = meta {
            if !m.file_type().is_symlink() {
                return Err(Error::msg(format!(
                    "{} 已存在但不是符号链接，请手动处理后重试",
                    link.display()
                )));
            }
            std::fs::remove_file(&link)?;
        }
        std::os::unix::fs::symlink(target, &link)?;
    }
    Ok(())
}

/// 把当前可执行文件复制为 `bin/` 下的各命令 shim；CLI 本体（文件名 jvm）
/// 运行时还会把自身复制为 `bin/jvm`，与 shim 同目录，用户 PATH 只需挂
/// `bin/` 一个条目即可用上全部命令。
///
/// shim 与 GUI/CLI 共用分发逻辑：入口按文件名分发（文件名是 java/javac
/// 等时进入 shim 模式），所以无论哪个二进制被复制为 shim 都能分发。
/// 但拷贝源统一为 CLI 本体：GUI 是 GUI 子系统进程（无控制台），作为
/// shim 转发输出要 AttachConsole 且管道/重定向下 stdio 语义不可靠
/// （实测输出乱序甚至丢失），体积也大一倍以上。目标已存在且体积与
/// 源一致时跳过，供应用启动时的同步零开销复用。
pub fn write_shims(home: &Home) -> Result<Vec<PathBuf>> {
    write_shims_from(&resolve_shim_source(&std::env::current_exe()?), home)
}

/// 决定 shim 拷贝源：CLI 本体用自身；GUI 壳优先取同目录的 CLI 二进制
/// （打包布局两壳同目录），找不到才退回自身（shim_entry 的
/// attach_parent_console 为该退化路径兜底）。
fn resolve_shim_source(exe: &std::path::Path) -> std::path::PathBuf {
    if is_cli_exe(exe) {
        return exe.to_path_buf();
    }
    sibling_cli_exe(exe).unwrap_or_else(|| exe.to_path_buf())
}

/// exe 同目录下的 CLI 本体（jvm.exe）；不存在时返回 None。
fn sibling_cli_exe(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let cli = exe.parent()?.join(installer::exe_name(CLI_COMMAND));
    cli.is_file().then_some(cli)
}

/// 同 [`write_shims`]，但源 exe 由调用方指定（测试注入用）。
pub fn write_shims_from(exe: &std::path::Path, home: &Home) -> Result<Vec<PathBuf>> {
    let src_len = std::fs::metadata(exe).map(|m| m.len()).unwrap_or(0);
    let bin = home.bin();
    std::fs::create_dir_all(&bin)?;
    let mut written = Vec::new();
    let mut commands: Vec<&'static str> = SHIM_TOOLS.to_vec();
    if is_cli_exe(exe) {
        // 当前进程就是 bin/jvm 自身时，下方按路径相等跳过，不会覆盖运行中的 exe。
        commands.push(CLI_COMMAND);
    }
    for tool in commands {
        let dest = bin.join(installer::exe_name(tool));
        if dest == exe {
            continue;
        }
        if src_len > 0
            && std::fs::metadata(&dest)
                .map(|m| m.len() == src_len)
                .unwrap_or(false)
        {
            continue;
        }
        match std::fs::copy(exe, &dest) {
            Ok(_) => written.push(dest),
            Err(e) => {
                // Windows 下该 shim 正被运行时会拒绝覆盖：shim 进程极短，
                // 重试一次，仍失败则告警继续（其余命令仍可用）。
                std::thread::sleep(std::time::Duration::from_millis(200));
                match std::fs::copy(exe, &dest) {
                    Ok(_) => written.push(dest),
                    Err(_) => tracing::warn!("shim {} 写入失败: {e}", dest.display()),
                }
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
        }
    }
    if written.is_empty() {
        tracing::debug!("命令 shim 均为最新，无需重建");
    } else {
        tracing::info!(count = written.len(), dir = %bin.display(), "已重建命令 shim");
    }
    Ok(written)
}

/// 卸载前检查：默认版本或被项目绑定则拒绝。
pub fn ensure_uninstallable(home: &Home, id: &JdkId) -> Result<()> {
    if store::read_default(home)?.as_ref() == Some(id) {
        return Err(Error::InUse(id.to_string()));
    }
    let projects = store::load_projects(home)?;
    if projects.count_bindings(&id.to_string()) > 0 {
        return Err(Error::InUse(id.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::exe_name;

    fn tmp_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_root(dir.path());
        home.ensure_dirs().unwrap();
        (dir, home)
    }

    #[test]
    fn current_link_and_shims() {
        let (_d, home) = tmp_home();
        let target = home.versions().join("temurin_17.0.12_7");
        std::fs::create_dir_all(&target).unwrap();

        rebuild_current_link(&home, &target).unwrap();
        #[cfg(windows)]
        assert!(junction::exists(home.current()).unwrap());
        #[cfg(unix)]
        assert!(std::fs::symlink_metadata(home.current())
            .unwrap()
            .file_type()
            .is_symlink());

        // 重复重建（切换到新版本时的正常路径）
        rebuild_current_link(&home, &target).unwrap();

        // current 指向的内容与目标一致
        let meta = std::fs::metadata(home.current()).unwrap();
        assert!(meta.is_dir());
    }

    #[test]
    fn write_shims_copies_current_exe() {
        let (_d, home) = tmp_home();
        let written = write_shims(&home).unwrap();
        // 7 个工具中至少 6 个（java/jar/jshell 等与本进程文件名不同）被写入
        assert!(written.len() >= SHIM_TOOLS.len() - 1);
        let java_shim = home.bin().join(exe_name("java"));
        assert!(java_shim.exists());
        let size_a = std::fs::metadata(&java_shim).unwrap().len();
        let size_b = std::fs::metadata(std::env::current_exe().unwrap()).unwrap().len();
        assert_eq!(size_a, size_b);
    }

    #[test]
    fn write_shims_skips_unchanged() {
        let (_d, home) = tmp_home();
        assert!(!write_shims(&home).unwrap().is_empty());
        // 第二次写入：目标体积与当前 exe 一致，全部跳过
        assert!(write_shims(&home).unwrap().is_empty());
    }

    #[test]
    fn cli_exe_writes_self_into_bin() {
        let (_d, home) = tmp_home();
        // 伪造名为 jvm 的 CLI 本体（内容用测试二进制即可）
        let fake_cli = _d.path().join(exe_name("jvm"));
        std::fs::copy(std::env::current_exe().unwrap(), &fake_cli).unwrap();
        let written = write_shims_from(&fake_cli, &home).unwrap();
        assert_eq!(written.len(), managed_commands().count());
        assert!(home.bin().join(exe_name("jvm")).exists());
    }

    #[test]
    fn non_cli_exe_never_writes_jvm_self() {
        let (_d, home) = tmp_home();
        // GUI 壳（JVMDesktop.exe）只同步 7 个工具 shim，不写 bin/jvm：
        // 否则 `jvm` 命令会按文件名分发成 GUI 而不是 CLI。
        let fake_gui = _d.path().join("JVMDesktop.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &fake_gui).unwrap();
        let written = write_shims_from(&fake_gui, &home).unwrap();
        assert_eq!(written.len(), SHIM_TOOLS.len());
        assert!(!home.bin().join(exe_name("jvm")).exists());
    }

    #[test]
    fn shim_source_prefers_sibling_cli() {
        let d = tempfile::tempdir().unwrap();
        let cli = d.path().join(exe_name("jvm"));
        std::fs::write(&cli, b"cli").unwrap();
        // CLI 本体：用自身
        assert_eq!(resolve_shim_source(&cli), cli);
        // GUI 壳：同目录存在 CLI 本体时优先用它作拷贝源
        let gui = d.path().join("JVMDesktop.exe");
        std::fs::write(&gui, b"gui").unwrap();
        assert_eq!(resolve_shim_source(&gui), cli);
        // 同目录没有 CLI 本体时退回自身（attach_parent_console 兜底）
        let d2 = tempfile::tempdir().unwrap();
        let gui2 = d2.path().join("JVMDesktop.exe");
        std::fs::write(&gui2, b"gui").unwrap();
        assert_eq!(resolve_shim_source(&gui2), gui2);
    }

    #[test]
    fn uninstall_guard() {
        let (_d, home) = tmp_home();
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        store::write_default(&home, &id).unwrap();
        assert!(matches!(
            ensure_uninstallable(&home, &id),
            Err(Error::InUse(_))
        ));
        store::write_default(&home, &JdkId::parse("zulu@21.0.1+12").unwrap()).unwrap();
        ensure_uninstallable(&home, &id).unwrap();

        let mut projects = store::Projects::default();
        projects.upsert(store::ProjectEntry {
            path: "D:/work".into(),
            name: "w".into(),
            jdk: id.to_string(),
            added_at: "t".into(),
            last_used_at: "t".into(),
        });
        store::save_projects(&home, &projects).unwrap();
        assert!(matches!(
            ensure_uninstallable(&home, &id),
            Err(Error::InUse(_))
        ));
    }
}
