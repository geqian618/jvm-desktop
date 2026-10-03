//! 终端输出辅助：着色与人类可读格式。

use console::style;

pub fn ok(text: impl std::fmt::Display) -> String {
    style(format!("[ok] {text}")).green().to_string()
}

pub fn warn(text: impl std::fmt::Display) -> String {
    style(format!("[警告] {text}")).yellow().to_string()
}

pub fn fail(text: impl std::fmt::Display) -> String {
    style(text).red().bold().to_string()
}

pub fn dim(text: impl std::fmt::Display) -> String {
    style(text).dim().to_string()
}

pub fn accent(text: impl std::fmt::Display) -> String {
    style(text).cyan().bold().to_string()
}

/// 人类可读大小：`190817615 → 182.0 MB`。
pub fn human_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(190_817_615), "182.0 MB");
        assert!(human_size(2 * 1024 * 1024 * 1024).ends_with("GB"));
    }
}
