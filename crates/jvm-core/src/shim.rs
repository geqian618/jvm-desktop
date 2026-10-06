//! shim 运行时：busybox 式单二进制，按 argv[0] 分发到对应 JDK 工具。
//!
//! shim 保持零 tokio、零日志（每次 `java` 调用都会走这里，必须快）。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::home::Home;
use crate::installer;
use crate::project;
use crate::store;

/// 当前 exe 的文件名是否命中 shim 命令集（`java` / `javac` / …）。
pub fn is_shim_tool(stem: &str) -> bool {
    crate::switcher::SHIM_TOOLS
        .iter()
        .any(|t| t.eq_ignore_ascii_case(stem))
}

/// 解析在 `cwd` 下执行 `tool` 应使用的二进制与其 JDK home。
pub fn resolve_tool(home: &Home, cwd: &Path, tool: &str) -> Result<(PathBuf, PathBuf)> {
    let resolution = project::resolve(cwd, home).ok_or_else(|| {
        Error::msg("未找到生效的 JDK：当前目录无绑定且未设置默认版本，请先 `jvm use <版本>`")
    })?;
    let id = resolution.id();
    let index = store::load_index(home)?;
    let rec = index
        .get(&id.to_string())
        .ok_or_else(|| Error::NotFound(id.to_string()))?;
    let dir = store::resolve_dir(home, &rec.dir);
    let jdk_home = installer::jdk_home_of(&dir).ok_or_else(|| {
        Error::IndexOutOfSync(format!(
            "版本 {} 的目录 {} 缺少 bin/java，可重新安装",
            id,
            dir.display()
        ))
    })?;
    let bin = jdk_home.join("bin").join(installer::exe_name(tool));
    if !bin.is_file() {
        return Err(Error::msg(format!(
            "{tool} 在 {id} 中不可用（{} 不存在）",
            bin.display()
        )));
    }
    Ok((bin, jdk_home))
}

/// shim 入口：转发到解析出的工具；返回进程退出码。
pub fn run_shim(home: &Home, tool: &str) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("jvm-shim: 无法获取当前目录: {e}");
            return 1;
        }
    };
    match resolve_tool(home, &cwd, tool) {
        Ok((bin, jdk_home)) => {
            // 为子进程注入 JAVA_HOME：mvn/gradle 等构建工具直接读它，
            // 这样项目级绑定不依赖 shell 里的全局变量也能生效。
            std::env::set_var("JAVA_HOME", &jdk_home);
            let args: Vec<String> = std::env::args().skip(1).collect();
            forward(&bin, &args)
        }
        Err(e) => {
            eprintln!("jvm-shim: {e}");
            1
        }
    }
}

fn forward(bin: &Path, args: &[String]) -> i32 {
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args);
    #[cfg(unix)]
    {
        // Unix：exec 替换当前进程，信号与退出码语义与直接运行一致。
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!("jvm-shim: exec 失败: {err}");
        127
    }
    #[cfg(windows)]
    {
        match cmd.status() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                eprintln!("jvm-shim: 启动 {} 失败: {e}", bin.display());
                127
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::JdkId;
    use crate::store::InstalledJdk;

    fn tmp_home() -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_root(dir.path());
        home.ensure_dirs().unwrap();
        (dir, home)
    }

    #[test]
    fn shim_tool_names() {
        assert!(is_shim_tool("java"));
        assert!(is_shim_tool("JAVA"));
        assert!(is_shim_tool("jshell"));
        assert!(!is_shim_tool("jvm"));
        assert!(!is_shim_tool("mvn"));
    }

    #[test]
    fn resolve_tool_walks_jvmrc() {
        let (d, home) = tmp_home();
        // 构造版本目录与索引
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        let dir = home.version_dir(&id);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join(installer::exe_name("java")), b"x").unwrap();
        let mut index = store::Index::default();
        index.upsert(InstalledJdk {
            id: id.to_string(),
            name: "temurin-17.0.12+7".into(),
            vendor: "temurin".into(),
            version: "17.0.12+7".into(),
            feature: 17,
            lts: true,
            dir: id.to_dir_name(),
            java_version_output: String::new(),
            installed_at: "t".into(),
        });
        store::save_index(&home, &index).unwrap();
        store::write_default(&home, &id).unwrap();

        let sub = d.path().join("proj").join("src");
        std::fs::create_dir_all(&sub).unwrap();
        project::write_jvmrc(&d.path().join("proj"), &id).unwrap();

        let (bin, jdk_home) = resolve_tool(&home, &sub, "java").unwrap();
        assert_eq!(bin, dir.join("bin").join(installer::exe_name("java")));
        assert_eq!(jdk_home, dir);
    }

    #[test]
    fn resolve_tool_errors_without_default() {
        let (_d, home) = tmp_home();
        let err = resolve_tool(&home, _d.path(), "java").unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
