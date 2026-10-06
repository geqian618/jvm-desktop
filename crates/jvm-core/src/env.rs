//! 平台环境变量适配层。
//!
//! 平台差异（提示词要求在注释中说明原因）：
//! - Windows：写 HKCU\Environment（JAVA_HOME 为 REG_SZ 绝对路径；用户
//!   PATH 保留原注册表类型，因为用户 PATH 常为 REG_EXPAND_SZ，改类型会
//!   使 `%...%` 引用失效）。写后广播 WM_SETTINGCHANGE，让资源管理器与
//!   新进程立即看到新值；每次修改前把旧值备份到 logs/ 便于回滚。
//! - macOS/Linux：按提示词要求不自动改 rc 文件，只返回提示
//!   （`NoopEnvWriter`），由 CLI 打印引导。

use std::path::Path;

use crate::error::Result;

/// 用户环境变量写入器抽象。
pub trait UserEnvWriter {
    fn set(&mut self, name: &str, value: &str) -> Result<()>;
    /// 向用户 PATH 追加目录（幂等）。
    fn append_path_entry(&mut self, dir: &Path) -> Result<()>;
    /// 从用户 PATH 移除目录（不存在则不动；取消默认版本时清理 bin 目录用）。
    fn remove_path_entry(&mut self, dir: &Path) -> Result<()>;
    /// 删除用户环境变量（取消默认时清 JAVA_HOME；不存在则不动）。
    /// 默认无操作：仅 Windows 需要真实落盘，Unix 不自动改 rc。
    fn remove(&mut self, name: &str) -> Result<()> {
        let _ = name;
        Ok(())
    }
    /// 清理用户 PATH 中的同义重复条目（`%VAR%` 展开后指向同一目录的
    /// 只保留最先出现的一个），返回被移除的原始条目。默认无操作：
    /// Unix 不自动改 rc 文件，PATH 由用户自己维护。
    fn dedupe_path(&mut self) -> Result<Vec<String>> {
        let _ = self;
        Ok(Vec::new())
    }
}

/// 无操作实现（Unix / 测试）。
pub struct NoopEnvWriter;

impl UserEnvWriter for NoopEnvWriter {
    fn set(&mut self, _name: &str, _value: &str) -> Result<()> {
        Ok(())
    }
    fn append_path_entry(&mut self, _dir: &Path) -> Result<()> {
        Ok(())
    }
    fn remove_path_entry(&mut self, _dir: &Path) -> Result<()> {
        Ok(())
    }
}

/// 按当前平台返回环境变量写入器。
pub fn platform_writer(logs_dir: &Path) -> Box<dyn UserEnvWriter> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsUserEnv::new(logs_dir))
    }
    #[cfg(not(windows))]
    {
        let _ = logs_dir;
        Box::new(NoopEnvWriter)
    }
}

// ---------- Windows 实现 ----------

#[cfg(windows)]
pub mod windows {
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
    use winreg::{RegKey, RegValue};

    use super::UserEnvWriter;
    use crate::error::{Error, Result};

    pub struct WindowsUserEnv {
        logs_dir: PathBuf,
    }

    impl WindowsUserEnv {
        pub fn new(logs_dir: &Path) -> Self {
            Self {
                logs_dir: logs_dir.to_path_buf(),
            }
        }

        fn env_key() -> Result<RegKey> {
            RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey_with_flags("Environment", KEY_QUERY_VALUE | KEY_SET_VALUE)
                .map_err(|e| Error::msg(format!("打开注册表 HKCU\\Environment 失败: {e}")))
        }

        /// 把修改前的旧值备份到 logs/，失败可人工回滚。
        fn backup(&self, name: &str, old: Option<&RegValue>) {
            if let Some(v) = old {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let file = self.logs_dir.join(format!("env-backup-{name}-{ts}.txt"));
                let _ = std::fs::create_dir_all(&self.logs_dir);
                let _ = std::fs::write(
                    file,
                    format!(
                        "name={name}\ntype={:?}\nvalue={}\n",
                        v.vtype,
                        decode_utf16(&v.bytes)
                    ),
                );
            }
        }

        fn unchanged(old: Option<&RegValue>, value: &str) -> bool {
            old.map(|v| decode_utf16(&v.bytes).eq_ignore_ascii_case(value))
                .unwrap_or(false)
        }

        /// 按条目列表重建用户 PATH：保留原注册表类型（REG_EXPAND_SZ 里
        /// 的 %...% 引用不能因改类型而失效），写前备份旧值，写后广播。
        fn write_path(&self, old: Option<&RegValue>, entries: &[String]) -> Result<()> {
            let vtype = old
                .map(|v| v.vtype.clone())
                .unwrap_or(RegType::REG_EXPAND_SZ);
            let new_text = entries.join(";");
            let bytes: Vec<u8> = new_text
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .flat_map(|u| u.to_le_bytes())
                .collect();
            let key = Self::env_key()?;
            self.backup("Path", old);
            key.set_raw_value("Path", &RegValue { vtype, bytes })
                .map_err(|e| Error::msg(format!("写入用户 PATH 失败: {e}")))?;
            broadcast_env_change();
            Ok(())
        }
    }

    impl UserEnvWriter for WindowsUserEnv {
        fn set(&mut self, name: &str, value: &str) -> Result<()> {
            let key = Self::env_key()?;
            let old = key.get_raw_value(name).ok();
            if Self::unchanged(old.as_ref(), value) {
                return Ok(());
            }
            self.backup(name, old.as_ref());
            key.set_value(name, &value)
                .map_err(|e| Error::msg(format!("写入注册表 {name} 失败: {e}")))?;
            broadcast_env_change();
            tracing::info!(name, value, "已更新用户环境变量");
            Ok(())
        }

        fn append_path_entry(&mut self, dir: &Path) -> Result<()> {
            let key = Self::env_key()?;
            let old = key.get_raw_value("Path").ok();
            let text = match &old {
                Some(v) => decode_utf16(&v.bytes),
                None => String::new(),
            };
            let dir_str = dir.to_string_lossy().to_string();
            let mut entries = path_entries(&text);
            if entries.iter().any(|e| path_entry_matches(e, &dir_str)) {
                return Ok(());
            }
            entries.push(dir_str);
            self.write_path(old.as_ref(), &entries)?;
            tracing::info!(dir = %dir.display(), "已把 shim 目录加入用户 PATH");
            Ok(())
        }

        fn remove_path_entry(&mut self, dir: &Path) -> Result<()> {
            let key = Self::env_key()?;
            let old = key.get_raw_value("Path").ok();
            let Some(v) = old else {
                return Ok(());
            };
            let entries = path_entries(&decode_utf16(&v.bytes));
            let want = dir.to_string_lossy().to_string();
            let kept: Vec<String> = entries
                .iter()
                .filter(|e| !path_entry_matches(e, &want))
                .cloned()
                .collect();
            if kept.len() == entries.len() {
                return Ok(());
            }
            self.write_path(Some(&v), &kept)?;
            tracing::info!(dir = %dir.display(), "已从用户 PATH 移除目录");
            Ok(())
        }

        fn dedupe_path(&mut self) -> Result<Vec<String>> {
            let key = Self::env_key()?;
            let old = key.get_raw_value("Path").ok();
            let Some(v) = old else {
                return Ok(Vec::new());
            };
            let entries = path_entries(&decode_utf16(&v.bytes));
            let removed = duplicate_path_entries(&entries);
            if removed.is_empty() {
                return Ok(removed);
            }
            let kept: Vec<String> = entries
                .iter()
                .filter(|e| !removed.iter().any(|r| r == *e))
                .cloned()
                .collect();
            self.write_path(Some(&v), &kept)?;
            tracing::info!(count = removed.len(), "已去除用户 PATH 中的同义重复条目");
            Ok(removed)
        }

        fn remove(&mut self, name: &str) -> Result<()> {
            let key = Self::env_key()?;
            let old = key.get_raw_value(name).ok();
            let Some(v) = old else {
                return Ok(());
            };
            self.backup(name, Some(&v));
            key.delete_value(name)
                .map_err(|e| Error::msg(format!("删除用户环境变量 {name} 失败: {e}")))?;
            broadcast_env_change();
            tracing::info!(name, "已删除用户环境变量");
            Ok(())
        }
    }

    /// 解码注册表 UTF-16LE 字符串（含结尾 nul）。
    pub fn decode_utf16(bytes: &[u8]) -> String {
        let (pairs, _tail) = bytes.as_chunks::<2>();
        let units: Vec<u16> = pairs
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    }

    pub fn path_entries(text: &str) -> Vec<String> {
        text.split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// 展开 PATH 条目里的 `%VAR%` 段（用户 PATH 常用变量形式，如
    /// `%JVM_HOME%\bin`）；未知变量整段原样保留。比较前先展开，避免
    /// 「变量条目 + 字面条目」指向同一目录却被判为不同而重复添加。
    pub fn expand_path_entry(entry: &str) -> String {
        let mut out = String::with_capacity(entry.len());
        let mut rest = entry;
        while let Some(i) = rest.find('%') {
            let after = &rest[i + 1..];
            match after.find('%') {
                Some(j) => {
                    out.push_str(&rest[..i]);
                    match std::env::var(&after[..j]) {
                        Ok(v) => out.push_str(&v),
                        // 变量不存在时连百分号一起原样保留
                        Err(_) => out.push_str(&rest[i..i + j + 2]),
                    }
                    rest = &after[j + 1..];
                }
                None => break,
            }
        }
        out.push_str(rest);
        out
    }

    /// PATH 条目与目标目录是否同义（先展开 %VAR% 再做分隔符/大小写无关比较）。
    fn path_entry_matches(entry: &str, dir: &str) -> bool {
        expand_path_entry(entry)
            .replace('/', "\\")
            .eq_ignore_ascii_case(dir.replace('/', "\\").as_str())
    }

    /// 找出 PATH 条目里的同义重复：`%VAR%` 展开后指向同一目录的条目视为
    /// 重复（分隔符方向与大小写无关），保留最先出现者，返回后续重复项
    /// 的原始文本。供体检检测与 [`UserEnvWriter::dedupe_path`] 共用。
    pub fn duplicate_path_entries(entries: &[String]) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut removed = Vec::new();
        for e in entries {
            let key = expand_path_entry(e).replace('/', "\\").to_ascii_lowercase();
            if seen.contains(&key) {
                removed.push(e.clone());
            } else {
                seen.push(key);
            }
        }
        removed
    }

    /// 广播环境变量变更。
    ///
    /// 用 SendNotifyMessage（异步）而非 SendMessage：不必等待所有窗口
    /// 应答，避免被无响应窗口阻塞。
    pub fn broadcast_env_change() {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendNotifyMessageW, WM_SETTINGCHANGE};
        // HWND_BROADCAST = 0xffff；windows-sys 不同版本句柄类型不同，
        // 用整型转换规避版本差异。
        let hwnd_broadcast: usize = 0xffff;
        let mut msg: Vec<u16> = "Environment\0".encode_utf16().collect();
        unsafe {
            SendNotifyMessageW(
                hwnd_broadcast as HWND,
                WM_SETTINGCHANGE,
                0,
                msg.as_mut_ptr() as isize,
            );
        }
    }

    /// 读取用户环境变量（doctor 用）。
    pub fn get_user_env(name: &str) -> Option<String> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Environment")
            .ok()?;
        key.get_raw_value(name).ok().map(|v| decode_utf16(&v.bytes))
    }

    /// 用户 PATH 是否已包含某目录（不区分大小写与分隔符方向）。
    pub fn user_path_contains(dir: &Path) -> bool {
        let Some(text) = get_user_env("Path") else {
            return false;
        };
        let want = dir.to_string_lossy().to_string();
        path_entries(&text)
            .iter()
            .any(|e| path_entry_matches(e, &want))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn decode_roundtrip() {
            let text = "C:\\Users\\test;D:\\tools\\bin";
            let bytes: Vec<u8> = text
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .flat_map(|u| u.to_le_bytes())
                .collect();
            assert_eq!(decode_utf16(&bytes), text);
        }

        #[test]
        fn expands_path_entry_vars() {
            std::env::set_var("JVM_TEST_HOME_X", "D:\\tools\\jvm");
            assert_eq!(
                expand_path_entry("%JVM_TEST_HOME_X%\\bin"),
                "D:\\tools\\jvm\\bin"
            );
            assert_eq!(expand_path_entry("C:\\static"), "C:\\static");
            // 未知变量与孤立百分号原样保留
            assert_eq!(expand_path_entry("%NOPE_XYZ%\\bin"), "%NOPE_XYZ%\\bin");
            assert_eq!(expand_path_entry("100%"), "100%");
        }

        #[test]
        fn detects_duplicate_path_entries() {
            std::env::set_var("JVM_TEST_HOME_X", "D:\\tools\\jvm");
            let entries: Vec<String> = vec![
                "%JVM_TEST_HOME_X%\\bin".into(),
                "D:/tools/jvm/bin".into(),    // 同义：分隔符方向不同
                "d:\\Tools\\JVM\\bin".into(), // 同义：大小写不同
                "C:\\other".into(),
            ];
            // 同义条目保留最先出现者（变量形式优先），后续重复项原样返回
            assert_eq!(
                duplicate_path_entries(&entries),
                vec![
                    "D:/tools/jvm/bin".to_string(),
                    "d:\\Tools\\JVM\\bin".to_string()
                ]
            );
            assert_eq!(
                duplicate_path_entries(&["C:\\a".to_string()]),
                Vec::<String>::new()
            );
        }

        #[test]
        fn path_entries_split() {
            assert_eq!(
                path_entries("C:\\a;  ;C:\\b"),
                vec!["C:\\a".to_string(), "C:\\b".to_string()]
            );
            assert!(path_entries("").is_empty());
        }
    }
}
