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
    /// 从用户 PATH 移除目录（不存在则不动；`.jdm` → `.jvm` 数据迁移用）。
    fn remove_path_entry(&mut self, dir: &Path) -> Result<()>;
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

    use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, RegType};
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
                let file = self
                    .logs_dir
                    .join(format!("env-backup-{name}-{ts}.txt"));
                let _ = std::fs::create_dir_all(&self.logs_dir);
                let _ = std::fs::write(
                    file,
                    format!("name={name}\ntype={:?}\nvalue={}\n", v.vtype, decode_utf16(&v.bytes)),
                );
            }
        }

        fn unchanged(old: Option<&RegValue>, value: &str) -> bool {
            old.map(|v| decode_utf16(&v.bytes).eq_ignore_ascii_case(value))
                .unwrap_or(false)
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
            let (vtype, text) = match &old {
                Some(v) => (v.vtype.clone(), decode_utf16(&v.bytes)),
                None => (RegType::REG_EXPAND_SZ, String::new()),
            };
            let dir_str = dir.to_string_lossy().to_string();
            if path_entries(&text)
                .iter()
                .any(|e| e.eq_ignore_ascii_case(&dir_str))
            {
                return Ok(());
            }
            self.backup("Path", old.as_ref());
            let new_text = if text.trim().is_empty() {
                dir_str
            } else {
                format!("{text};{dir_str}")
            };
            // 保留原注册表类型：用户 PATH 若为 REG_EXPAND_SZ，改成 REG_SZ
            // 会使其中 %...% 引用不再展开。
            let bytes: Vec<u8> = new_text
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .flat_map(|u| u.to_le_bytes())
                .collect();
            key.set_raw_value("Path", &RegValue { vtype, bytes })
                .map_err(|e| Error::msg(format!("写入用户 PATH 失败: {e}")))?;
            broadcast_env_change();
            tracing::info!(dir = %dir.display(), "已把 shim 目录加入用户 PATH");
            Ok(())
        }

        fn remove_path_entry(&mut self, dir: &Path) -> Result<()> {
            let key = Self::env_key()?;
            let old = key.get_raw_value("Path").ok();
            let Some(v) = old else {
                return Ok(());
            };
            let text = decode_utf16(&v.bytes);
            let entries = path_entries(&text);
            let want = dir.to_string_lossy().replace('/', "\\");
            let kept: Vec<String> = entries
                .iter()
                .filter(|e| !e.replace('/', "\\").eq_ignore_ascii_case(&want))
                .cloned()
                .collect();
            if kept.len() == entries.len() {
                return Ok(());
            }
            self.backup("Path", Some(&v));
            let new_text = kept.join(";");
            // 保留原注册表类型，理由同 append_path_entry。
            let bytes: Vec<u8> = new_text
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .flat_map(|u| u.to_le_bytes())
                .collect();
            key.set_raw_value("Path", &RegValue { vtype: v.vtype, bytes })
                .map_err(|e| Error::msg(format!("写入用户 PATH 失败: {e}")))?;
            broadcast_env_change();
            tracing::info!(dir = %dir.display(), "已从用户 PATH 移除目录");
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

    /// 广播环境变量变更。
    ///
    /// 用 SendNotifyMessage（异步）而非 SendMessage：不必等待所有窗口
    /// 应答，避免被无响应窗口阻塞。
    pub fn broadcast_env_change() {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendNotifyMessageW, WM_SETTINGCHANGE,
        };
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
        key.get_raw_value(name)
            .ok()
            .map(|v| decode_utf16(&v.bytes))
    }

    /// 用户 PATH 是否已包含某目录（不区分大小写与分隔符方向）。
    pub fn user_path_contains(dir: &Path) -> bool {
        let Some(text) = get_user_env("Path") else {
            return false;
        };
        let want = dir.to_string_lossy().replace('/', "\\");
        path_entries(&text)
            .iter()
            .any(|e| e.replace('/', "\\").eq_ignore_ascii_case(&want))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn decode_roundtrip() {
            let text = "C:\\Users\\test;.jdm\\bin";
            let bytes: Vec<u8> = text
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .flat_map(|u| u.to_le_bytes())
                .collect();
            assert_eq!(decode_utf16(&bytes), text);
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
