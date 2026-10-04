//! 安装器：归档解压、根目录探测、`java -version` 验证、本地 JDK 导入。
//!
//! 同步实现（CPU/IO 密集），调用方（CLI 直接调用；Tauri 侧 spawn_blocking）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::home::Home;
use crate::model::JdkId;

/// 验证超时：损坏的二进制可能挂死，超时即判定失败。
const VERIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// 安装结果：目录名、JDK home（macOS 下为 `Contents/Home`）、验证输出。
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub dir_name: String,
    pub jdk_home: PathBuf,
    pub java_version_output: String,
}

/// 可注入的验证函数：生产实现运行 `bin/java -version`，测试用桩。
pub type Verifier = dyn Fn(&Path) -> Result<String> + Send + Sync;

/// Windows 下可执行文件的 `.exe` 后缀；其余平台原样。
pub fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// 探测 JDK 实际 home：兼容标准布局 `<dir>/bin/java` 与 macOS 的
/// `<dir>/Contents/Home/bin/java`（Temurin 等发行商的 macOS 归档带
/// `Contents/Home` 一层）。
pub fn jdk_home_of(dir: &Path) -> Option<PathBuf> {
    [dir.to_path_buf(), dir.join("Contents").join("Home")]
        .into_iter()
        .find(|c| c.join("bin").join(exe_name("java")).is_file())
}

/// 生产验证器：运行 `bin/java -version`（注意其输出在 stderr）。
pub fn run_java_version(jdk_home: &Path) -> Result<String> {
    let java = jdk_home.join("bin").join(exe_name("java"));
    if !java.is_file() {
        return Err(Error::VerifyFailed(format!("{} 不存在", java.display())));
    }
    let mut cmd = Command::new(&java);
    cmd.arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // GUI（无控制台）父进程下启动控制台程序（java.exe）会闪现终端窗口，
        // CREATE_NO_WINDOW 抑制之；输出走管道不受影响。
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn()?;
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut text = String::new();
                if let Some(mut out) = child.stdout.take() {
                    use std::io::Read;
                    let _ = out.read_to_string(&mut text);
                }
                if let Some(mut err) = child.stderr.take() {
                    use std::io::Read;
                    let _ = err.read_to_string(&mut text);
                }
                if status.success() {
                    return Ok(text.trim().to_string());
                }
                return Err(Error::VerifyFailed(format!(
                    "退出码 {:?}，输出: {}",
                    status.code(),
                    text.trim()
                )));
            }
            None if start.elapsed() > VERIFY_TIMEOUT => {
                let _ = child.kill();
                return Err(Error::VerifyFailed("执行超时（疑似损坏的 JDK）".into()));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// 从已下载归档安装：解压 → 探测根目录与布局 → 验证 → 落位。
pub fn install_from_archive(
    archive: &Path,
    id: &JdkId,
    home: &Home,
    verify: &Verifier,
) -> Result<InstallOutcome> {
    let dir_name = id.to_dir_name();
    let versions = home.versions();
    std::fs::create_dir_all(&versions)?;
    let dest = versions.join(&dir_name);
    if dest.exists() {
        return Err(Error::AlreadyInstalled(id.to_string()));
    }
    let outcome = install_to_dir(archive, &dest, verify)?;
    Ok(InstallOutcome {
        dir_name,
        jdk_home: outcome.jdk_home,
        java_version_output: outcome.java_version_output,
    })
}

/// 解压归档到指定目标目录（GUI 自定义目录名时使用）。
pub fn install_to_dir(archive: &Path, dest: &Path, verify: &Verifier) -> Result<InstallOutcome> {
    tracing::info!(archive = %archive.display(), dest = %dest.display(), "开始解压安装");
    let parent = dest
        .parent()
        .ok_or_else(|| Error::msg(format!("目标目录缺少父级: {}", dest.display())))?;
    std::fs::create_dir_all(parent)?;
    let tmp_name = dest
        .file_name()
        .map(|s| format!(".tmp-{}", s.to_string_lossy()))
        .unwrap_or_else(|| ".tmp-jdk".to_string());
    let tmp = parent.join(tmp_name);

    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    let result = extract(archive, &tmp).and_then(|root| {
        let jdk_home = jdk_home_of(&root).ok_or(Error::BadArchive)?;
        place(&root, &tmp, dest)?;
        let moved = jdk_home_moved(&jdk_home, &root, dest);
        match verify(&moved) {
            Ok(output) => {
                tracing::info!(dest = %dest.display(), jdk_home = %moved.display(), "解压完成，java -version 验证通过");
                Ok(InstallOutcome {
                    dir_name: dest
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    jdk_home: moved,
                    java_version_output: output,
                })
            }
            Err(e) => {
                tracing::warn!(dest = %dest.display(), error = %e, "验证失败，回滚删除目标目录");
                let _ = std::fs::remove_dir_all(dest);
                Err(e)
            }
        }
    });

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    result
}

/// 解压并返回探测到的根目录（归档内一层目录，或解压根）。
fn extract(archive: &Path, into: &Path) -> Result<PathBuf> {
    let name = archive
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.ends_with(".zip") {
        extract_zip(archive, into)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_targz(archive, into)?;
    } else {
        return Err(Error::msg(format!(
            "不认识的归档格式: {name}（支持 .zip 与 .tar.gz）"
        )));
    }

    let mut entries: Vec<PathBuf> = std::fs::read_dir(into)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|e| e.path())
        .collect();
    // 常见归档内有一层 `jdk-17.0.12+7/` 目录：唯一顶层目录即为根。
    if entries.len() == 1 && entries[0].is_dir() {
        return Ok(entries.remove(0));
    }
    Ok(into.to_path_buf())
}

fn extract_zip(archive: &Path, into: &Path) -> Result<()> {
    // zip crate 的错误类型没有 From 到 Error，统一转成带上下文的消息。
    fn zerr(e: zip::result::ZipError) -> Error {
        Error::msg(format!("zip 解压失败: {e}"))
    }
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(zerr)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(zerr)?;
        // enclosed_name 防止 zip-slip（条目名含 ../ 逃逸）。
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out_path = into.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&out_path)?;
            std::io::copy(&mut entry, &mut out)?;
            // zip 条目可能携带 unix 权限位，bin/ 下的可执行文件依赖它。
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

fn extract_targz(archive: &Path, into: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut tar = tar::Archive::new(decoder);
    // tar 本身按 entry 路径解包，set_preserve_permissions 保留可执行位。
    tar.set_preserve_permissions(true);
    tar.unpack(into)?;
    Ok(())
}

/// 把根目录移动到最终位置。
fn place(root: &Path, tmp: &Path, dest: &Path) -> Result<()> {
    if root == tmp {
        std::fs::rename(tmp, dest)?;
    } else {
        std::fs::rename(root, dest)?;
        std::fs::remove_dir_all(tmp)?;
    }
    Ok(())
}

/// 根目录移动后，其内部探测到的 jdk_home 相应平移。
fn jdk_home_moved(jdk_home: &Path, old_root: &Path, new_root: &Path) -> PathBuf {
    match jdk_home.strip_prefix(old_root) {
        Ok(rel) => new_root.join(rel),
        Err(_) => new_root.to_path_buf(),
    }
}

/// 导入本地已有 JDK：拷贝进 versions/（卸载语义统一为删除目录，
/// 原地登记会导致卸载误删用户原始目录，故 v1 采用拷贝）。
pub fn import_directory(
    src: &Path,
    id: &JdkId,
    home: &Home,
    verify: &Verifier,
) -> Result<InstallOutcome> {
    tracing::info!(src = %src.display(), id = %id, "开始导入本地 JDK");
    let dir_name = id.to_dir_name();
    let versions = home.versions();
    std::fs::create_dir_all(&versions)?;
    let dest = versions.join(&dir_name);
    if dest.exists() {
        return Err(Error::AlreadyInstalled(id.to_string()));
    }
    if !src.is_dir() {
        return Err(Error::msg(format!("{} 不是目录", src.display())));
    }

    // src 本身是 JDK home，或其唯一子目录是（用户常选到外层）。
    let layout = if jdk_home_of(src).is_some() {
        src.to_path_buf()
    } else {
        let children: Vec<PathBuf> = std::fs::read_dir(src)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        match children.first() {
            Some(c) if children.len() == 1 && jdk_home_of(c).is_some() => c.clone(),
            _ => return Err(Error::BadArchive),
        }
    };

    let jdk_home_rel = jdk_home_of(&layout)
        .and_then(|h| h.strip_prefix(&layout).ok().map(|r| r.to_path_buf()))
        .unwrap_or_default();
    copy_tree(&layout, &dest)?;
    tracing::info!(dest = %dest.display(), "拷贝完成，开始验证");
    let moved_home = dest.join(&jdk_home_rel);
    match verify(&moved_home) {
        Ok(output) => Ok(InstallOutcome {
            dir_name,
            jdk_home: moved_home,
            java_version_output: output,
        }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dest);
            Err(e)
        }
    }
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)?;
        }
        // 符号链接跳过（M1：导入含 symlink 的 JDK 属边缘场景）。
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn stub_verifier(expect_jdk_home_has_bin: bool) -> impl Fn(&Path) -> Result<String> {
        move |home: &Path| {
            if home.join("bin").join(exe_name("java")).is_file() || !expect_jdk_home_has_bin {
                Ok("openjdk version \"17.0.12\" (stub)".to_string())
            } else {
                Err(Error::VerifyFailed("no bin/java".into()))
            }
        }
    }

    /// 构造一个 fake JDK zip：<root>/bin/java(.exe)。
    fn make_fake_zip(path: &Path, root: &str) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zip.add_directory(format!("{root}/bin"), opts).unwrap();
        zip.start_file(format!("{root}/bin/{}", exe_name("java")), opts).unwrap();
        zip.write_all(b"fake java").unwrap();
        zip.start_file(format!("{root}/release"), opts).unwrap();
        zip.write_all(b"JAVA_VERSION=\"17.0.12\"").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn install_zip_with_single_root() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::from_root(tmp.path().join(".jvm"));
        home.ensure_dirs().unwrap();
        let archive = tmp.path().join("jdk.zip");
        make_fake_zip(&archive, "jdk-17.0.12+7");

        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        let out = install_from_archive(&archive, &id, &home, &stub_verifier(true)).unwrap();
        assert_eq!(out.dir_name, "temurin@17.0.12+7");
        let dest = home.version_dir(&id);
        assert!(dest.join("bin").join(exe_name("java")).is_file(), "根目录应被剥离");
        assert!(dest.join("release").is_file());
        assert!(out.java_version_output.contains("stub"));
        // 临时目录应清理
        assert!(!home.versions().join(".tmp-temurin_17.0.12_7").exists());
    }

    #[test]
    fn install_rejects_existing_and_bad_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::from_root(tmp.path().join(".jvm"));
        home.ensure_dirs().unwrap();
        let archive = tmp.path().join("jdk.zip");
        make_fake_zip(&archive, "jdk-17.0.12+7");
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();

        install_from_archive(&archive, &id, &home, &stub_verifier(true)).unwrap();
        assert!(matches!(
            install_from_archive(&archive, &id, &home, &stub_verifier(true)),
            Err(Error::AlreadyInstalled(_))
        ));

        // 无 bin/java 的 zip → BadArchive
        let bad = tmp.path().join("bad.zip");
        make_fake_zip(&bad, "not-a-jdk-empty");
        // 该归档其实有 bin/java……再造一个真正缺 bin 的：
        let file = std::fs::File::create(tmp.path().join("empty.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zip.start_file("readme.txt", opts).unwrap();
        zip.write_all(b"no bin here").unwrap();
        zip.finish().unwrap();
        let id2 = JdkId::parse("temurin@18.0.1+9").unwrap();
        assert!(matches!(
            install_from_archive(&tmp.path().join("empty.zip"), &id2, &home, &stub_verifier(true)),
            Err(Error::BadArchive)
        ));
    }

    #[test]
    fn tar_gz_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::from_root(tmp.path().join(".jvm"));
        home.ensure_dirs().unwrap();

        // 手工打包 tar.gz：<root>/bin/java
        let src = tmp.path().join("payload");
        std::fs::create_dir_all(src.join("jdk-25.0.1+8/bin")).unwrap();
        std::fs::write(src.join("jdk-25.0.1+8/bin").join(exe_name("java")), b"#!/bin/sh\n").unwrap();
        let tar_gz = tmp.path().join("jdk.tar.gz");
        let f = std::fs::File::create(&tar_gz).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut tb = tar::Builder::new(enc);
        tb.append_dir_all("jdk-25.0.1+8", src.join("jdk-25.0.1+8")).unwrap();
        tb.into_inner().unwrap().finish().unwrap();

        let id = JdkId::parse("zulu@25.0.1+8").unwrap();
        let out = install_from_archive(&tar_gz, &id, &home, &stub_verifier(true)).unwrap();
        assert_eq!(out.dir_name, "zulu@25.0.1+8");
        assert!(home.version_dir(&id).join("bin").join(exe_name("java")).is_file());
    }

    #[test]
    fn detect_macos_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("jdk");
        std::fs::create_dir_all(dir.join("Contents/Home/bin")).unwrap();
        std::fs::write(dir.join("Contents/Home/bin").join(exe_name("java")), b"x").unwrap();
        let home = jdk_home_of(&dir).unwrap();
        assert!(home.ends_with("Contents/Home"));
    }

    #[test]
    fn import_directory_copies_and_verifies() {
        let tmp = tempfile::tempdir().unwrap();
        let home = Home::from_root(tmp.path().join(".jvm"));
        home.ensure_dirs().unwrap();
        let src = tmp.path().join("my-jdk");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin").join(exe_name("java")), b"fake").unwrap();

        let id = JdkId::parse("corretto@11.0.24+8").unwrap();
        let out = import_directory(&src, &id, &home, &stub_verifier(true)).unwrap();
        assert_eq!(out.dir_name, "corretto@11.0.24+8");
        assert!(home.version_dir(&id).join("bin").join(exe_name("java")).is_file());
        // 原目录保持不动
        assert!(src.join("bin").join(exe_name("java")).is_file());
    }
}
