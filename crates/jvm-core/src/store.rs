//! 本地持久化：setting.json / index.json / projects.json / default。
//!
//! 所有 JSON 写入均为原子操作（先写 .tmp 再 rename），损坏或缺失的文件
//! 一律回退为默认值而不是让工具不可用。
//!
//! setting.json 采用 GUI（JDK Desktop）设计稿的字段模型：数据源是
//! foojay 兼容的 URL（registry），代理是单个字符串（空 = 直连），
//! 另有 JDK 安装目录 / 下载路径 / 额外扫描目录。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::home::Home;
use crate::model::JdkId;

// ---------- 通用 JSON 原子读写 ----------

pub fn load_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    serde_json::from_str(&text).map(Some).map_err(Error::from)
}

/// 带损坏兜底的加载：文件缺失或 JSON 损坏时回退为默认值并告警，
/// 而不是让整个工具不可用（用户可随时手动修复或删除重来）。
fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match load_json(path) {
        Ok(v) => Ok(v.unwrap_or_default()),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "配置文件损坏，已回退为默认值");
            Ok(T::default())
        }
    }
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value)?;
    std::fs::write(&tmp, text)?;
    // std::fs::rename 在 Windows 上对已存在的目标文件同样执行替换。
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---------- setting.json ----------

/// 数据源预设（GUI 下拉/按钮用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryPreset {
    pub name: String,
    pub url: String,
    /// 内置源（官方预置）：GUI 里可编辑名称/地址，但不可删除。
    #[serde(default)]
    pub built_in: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub language: String,
    /// 预留：light / dark / system（M4 主题接入）
    pub theme: String,
    /// JDK 安装目录；空 = <root>/versions
    pub install_dir: String,
    /// 「仅下载」压缩包保存目录；空 = <root>/downloads
    pub download_dir: String,
    /// 版本数据源：foojay disco 兼容 base URL；空 = 官方源
    pub registry: String,
    pub registry_presets: Vec<RegistryPreset>,
    /// HTTP 代理，如 http://127.0.0.1:7890；空 = 直连
    pub proxy: String,
    /// foojay 部署的 API Key（可选）
    pub source_api_key: String,
    /// 安装成功后自动删除下载目录里的压缩包（GUI 设置项）
    pub auto_clean_download: bool,
    /// 解除项目关联时同时删除项目目录里的 .jvmrc（GUI 设置项；
    /// 该文件可能已提交到 Git 供团队共享，默认保留）
    pub unbind_deletes_jvmrc: bool,
}

/// 内置数据源预设的出厂定义：`Settings::default()` 由此生成；编辑内置
/// 预设时名称/URL 留空也按此回退。GUI 侧在 SettingsView.vue 有一份镜像
/// 定义（BUILTIN_PRESET_DEFAULTS），改动需两处同步。
pub fn builtin_presets() -> [RegistryPreset; 2] {
    [
        RegistryPreset {
            name: "Foojay Disco (Official)".to_string(),
            url: crate::source::FOOJAY_BASE.to_string(),
            built_in: true,
        },
        RegistryPreset {
            name: "Adoptium (Temurin)".to_string(),
            url: crate::source::ADOPTIUM_BASE.to_string(),
            built_in: true,
        },
    ]
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "zh-CN".to_string(),
            theme: "system".to_string(),
            install_dir: String::new(),
            download_dir: String::new(),
            registry: String::new(),
            registry_presets: builtin_presets().to_vec(),
            proxy: String::new(),
            source_api_key: String::new(),
            auto_clean_download: false,
            unbind_deletes_jvmrc: false,
        }
    }
}

pub fn load_settings(home: &Home) -> Result<Settings> {
    let mut s: Settings = load_json_or_default(&home.settings_file())?;
    // 旧版 setting.json 的预设没有 built_in 标记：按官方 URL 补上，
    // 保证升级后内置源仍不可删除。
    for p in &mut s.registry_presets {
        let url = p.url.trim().trim_end_matches('/');
        if url == crate::source::FOOJAY_BASE || url == crate::source::ADOPTIUM_BASE {
            p.built_in = true;
        }
    }
    Ok(s)
}

pub fn save_settings(home: &Home, settings: &Settings) -> Result<()> {
    save_json(&home.settings_file(), settings)
}

/// GUI 整对象写回时的字段级合并：GUI 与 CLI 是同一份 setting.json 的两个
/// 写入者，GUI 持有的快照可能在落盘前已被 CLI 改过。`external_write` 为真
/// （磁盘内容 ≠ GUI 上次读取的快照）时，数据源字段以磁盘为准——CLI 的
/// 注册/编辑/切换不被陈旧快照抹掉；其余字段以 GUI 为准（设置页正在编辑
/// 的就是它们）。无并发写入时整体采用 GUI 对象，行为与从前一致。
pub fn merge_settings(incoming: Settings, disk: &Settings, external_write: bool) -> Settings {
    if !external_write {
        return incoming;
    }
    let mut merged = incoming;
    merged.registry = disk.registry.clone();
    merged.registry_presets = disk.registry_presets.clone();
    merged
}

// ---------- index.json ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InstalledJdk {
    pub id: String,
    /// 展示名（内部安装默认为目录名）
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub feature: u32,
    pub lts: bool,
    /// `versions/` 下的目录名。
    pub dir: String,
    /// 安装验证时 `java -version` 的输出，供 GUI/doctor 展示。
    pub java_version_output: String,
    pub installed_at: String,
}

impl InstalledJdk {
    /// 记录的唯一标识（`vendor@version`）。
    pub fn jdk_id(&self) -> JdkId {
        JdkId::parse(&self.id)
            .unwrap_or_else(|_| JdkId::new(self.vendor.clone(), self.version.clone()))
    }

    /// 安装时间的本地时区日期（YYYY-MM-DD）。installed_at 是 UTC 的
    /// RFC3339 串，直接截取前 10 位在跨日界时区会差一天，这里转本地日期；
    /// 为空或解析失败（旧索引/测试桩）返回 None。
    pub fn installed_date(&self) -> Option<String> {
        chrono::DateTime::parse_from_rfc3339(&self.installed_at)
            .ok()
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    pub version: u32,
    pub installed: Vec<InstalledJdk>,
}

impl Index {
    pub fn get(&self, id: &str) -> Option<&InstalledJdk> {
        self.installed.iter().find(|r| r.id == id)
    }

    pub fn upsert(&mut self, rec: InstalledJdk) {
        self.installed.retain(|r| r.id != rec.id);
        self.installed.push(rec);
    }

    pub fn remove(&mut self, id: &str) -> Option<InstalledJdk> {
        let pos = self.installed.iter().position(|r| r.id == id)?;
        Some(self.installed.remove(pos))
    }
}

/// 把索引记录的 dir 字段解析为实际路径：`versions/` 下的目录名。
pub fn resolve_dir(home: &Home, dir: &str) -> PathBuf {
    home.versions().join(dir)
}

pub fn load_index(home: &Home) -> Result<Index> {
    load_json_or_default(&home.index_file())
}

pub fn save_index(home: &Home, index: &Index) -> Result<()> {
    save_json(&home.index_file(), index)
}

/// index.json 是整文件覆盖式保存：并发安装/卸载若各自持旧快照回写，
/// 后写者会抹掉先写者的登记（lost update），因此所有「重读 → 修改 →
/// 写回」必须经 [`update_index`] 在全局锁内串行执行。
static INDEX_LOCK: Mutex<()> = Mutex::new(());

/// 在全局锁内重新加载索引、执行 `mutate` 并整体写回。
///
/// 调用方不得缓存 `Index` 快照后自行 `save_index`——那会绕过锁，
/// 重新引入丢失更新。
pub fn update_index<T>(home: &Home, mutate: impl FnOnce(&mut Index) -> Result<T>) -> Result<T> {
    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut index = load_index(home)?;
    let out = mutate(&mut index)?;
    save_index(home, &index)?;
    Ok(out)
}

// ---------- projects.json ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectEntry {
    /// 项目根绝对路径（统一 `/` 分隔存储）。
    pub path: String,
    pub name: String,
    /// 绑定的 JDK：`vendor@version`。
    pub jdk: String,
    pub added_at: String,
    pub last_used_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Projects {
    pub version: u32,
    pub projects: Vec<ProjectEntry>,
}

impl Projects {
    /// 找到覆盖 `dir` 的、路径最长的登记项目（即最近的项目根祖先）。
    /// 匹配不区分大小写与分隔符方向（Windows 文件系统不区分大小写）。
    pub fn nearest_binding(&self, dir: &Path) -> Option<&ProjectEntry> {
        let key = path_key(dir);
        self.projects
            .iter()
            .filter(|p| {
                let pk = path_key(Path::new(&p.path));
                key == pk || key.starts_with(&format!("{pk}/"))
            })
            .max_by_key(|p| path_key(Path::new(&p.path)).len())
    }

    /// 登记或更新项目绑定；返回是否为新项目。
    pub fn upsert(&mut self, entry: ProjectEntry) -> bool {
        let key = path_key(Path::new(&entry.path));
        if let Some(existing) = self
            .projects
            .iter_mut()
            .find(|p| path_key(Path::new(&p.path)) == key)
        {
            existing.jdk = entry.jdk;
            existing.last_used_at = entry.last_used_at;
            return false;
        }
        self.projects.push(entry);
        true
    }

    /// 某版本被多少项目绑定（卸载前检查用）。
    pub fn count_bindings(&self, id: &str) -> usize {
        self.projects.iter().filter(|p| p.jdk == id).count()
    }

    pub fn remove_by_path(&mut self, dir: &Path) -> bool {
        let key = path_key(dir);
        let before = self.projects.len();
        self.projects
            .retain(|p| path_key(Path::new(&p.path)) != key);
        before != self.projects.len()
    }
}

pub fn load_projects(home: &Home) -> Result<Projects> {
    load_json_or_default(&home.projects_file())
}

pub fn save_projects(home: &Home, projects: &Projects) -> Result<()> {
    save_json(&home.projects_file(), projects)
}

/// projects.json 与 index.json 一样是整文件覆盖式保存：所有「重读 → 修改
/// → 写回」必须经 [`update_projects`] 在全局锁内串行执行，否则 resolve 的
/// last_used_at 刷新与 bind/unbind 并发时会互相覆盖（lost update）。
static PROJECTS_LOCK: Mutex<()> = Mutex::new(());

/// 在全局锁内重新加载 projects、执行 `mutate` 并整体写回。
pub fn update_projects<T>(
    home: &Home,
    mutate: impl FnOnce(&mut Projects) -> Result<T>,
) -> Result<T> {
    let _guard = PROJECTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut projects = load_projects(home)?;
    let out = mutate(&mut projects)?;
    save_projects(home, &projects)?;
    Ok(out)
}

// ---------- default 文件 ----------

pub fn read_default(home: &Home) -> Result<Option<JdkId>> {
    match read_text(&home.default_file())? {
        Some(text) => {
            let line = text
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .unwrap_or("");
            if line.is_empty() {
                return Ok(None);
            }
            JdkId::parse(line).map(Some).map_err(|_| {
                Error::msg(format!(
                    "`default` 文件内容 `{line}` 不是合法的 vendor@version"
                ))
            })
        }
        None => Ok(None),
    }
}

pub fn write_default(home: &Home, id: &JdkId) -> Result<()> {
    write_text_atomic(&home.default_file(), &format!("{id}\n"))
}

/// 移除 default 登记（取消默认用）；文件不存在视为已移除。
pub fn remove_default(home: &Home) -> Result<()> {
    match std::fs::remove_file(home.default_file()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::msg(format!("删除 default 文件失败: {e}"))),
    }
}

// ---------- 路径归一化 ----------

/// 生成用于比较的路径键：绝对化、`/` 分隔、去 `\\?\` 前缀、Windows 下小写。
pub fn path_key(p: &Path) -> String {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    let mut s = abs.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = s.strip_prefix("//?/") {
        s = stripped.to_string();
    }
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::from_root(dir.path().join(tag));
        home.ensure_dirs().unwrap();
        (dir, home)
    }

    #[test]
    fn merge_settings_keeps_cli_source_changes() {
        // 磁盘：CLI 刚注册了一个镜像并切换过去
        let mut disk = Settings::default();
        disk.registry = "https://mirror.local/disco".into();
        disk.registry_presets.push(RegistryPreset {
            name: "内部镜像".into(),
            url: "https://mirror.local/disco".into(),
            built_in: false,
        });
        // GUI 的陈旧快照：源还是旧的，但用户改了代理
        let mut incoming = Settings::default();
        incoming.proxy = "http://127.0.0.1:7890".into();

        // 有外部写入：数据源字段回磁盘值，其余字段保留 GUI 值
        let merged = merge_settings(incoming.clone(), &disk, true);
        assert_eq!(merged.registry, "https://mirror.local/disco");
        assert_eq!(merged.registry_presets.len(), 3, "CLI 注册的预设保留");
        assert_eq!(merged.proxy, "http://127.0.0.1:7890");

        // 无外部写入：整体采用 GUI 对象（行为与从前一致）
        let merged = merge_settings(incoming, &disk, false);
        assert_eq!(merged.registry, "");
        assert_eq!(merged.registry_presets.len(), 2);
    }

    #[test]
    fn index_roundtrip_and_lookup() {
        let (_d, home) = tmp_home("idx");
        let mut index = Index::default();
        index.upsert(InstalledJdk {
            id: "temurin@17.0.12+7".into(),
            name: "temurin-17.0.12+7".into(),
            vendor: "temurin".into(),
            version: "17.0.12+7".into(),
            feature: 17,
            lts: true,
            dir: "temurin_17.0.12_7".into(),
            java_version_output: "openjdk version \"17.0.12\"".into(),
            installed_at: "2026-10-01T00:00:00Z".into(),
        });
        save_index(&home, &index).unwrap();

        let mut loaded = load_index(&home).unwrap();
        assert_eq!(loaded.installed.len(), 1);
        assert!(loaded.get("temurin@17.0.12+7").is_some());
        assert_eq!(loaded.remove("temurin@17.0.12+7").unwrap().feature, 17);
        assert!(loaded.get("temurin@17.0.12+7").is_none());
    }

    #[test]
    fn settings_default_when_missing() {
        let (_d, home) = tmp_home("settings");
        let s = load_settings(&home).unwrap();
        assert_eq!(s.language, "zh-CN");
        assert!(s.registry.is_empty());
        assert_eq!(s.registry_presets.len(), 2);
        assert!(s.registry_presets.iter().all(|p| p.built_in));
    }

    #[test]
    fn legacy_settings_presets_marked_built_in() {
        let (_d, home) = tmp_home("legacy");
        let legacy = r#"{
            "language": "zh-CN",
            "registryPresets": [
                { "name": "Foojay Disco (Official)", "url": "https://api.foojay.io/disco/v3.0/" },
                { "name": "我的镜像", "url": "https://mirror.local/disco" }
            ]
        }"#;
        std::fs::write(home.settings_file(), legacy).unwrap();
        let s = load_settings(&home).unwrap();
        assert!(s.registry_presets[0].built_in);
        assert!(!s.registry_presets[1].built_in);
    }

    #[test]
    fn default_file_roundtrip() {
        let (_d, home) = tmp_home("def");
        assert!(read_default(&home).unwrap().is_none());
        let id = JdkId::parse("temurin@17.0.12+7").unwrap();
        write_default(&home, &id).unwrap();
        assert_eq!(read_default(&home).unwrap().unwrap(), id);
    }

    #[test]
    fn projects_nearest_ancestor() {
        let mut p = Projects::default();
        p.upsert(ProjectEntry {
            path: "D:/work".into(),
            name: "work".into(),
            jdk: "temurin@21.0.4+7".into(),
            added_at: "t".into(),
            last_used_at: "t".into(),
        });
        p.upsert(ProjectEntry {
            path: "D:/work/demo".into(),
            name: "demo".into(),
            jdk: "temurin@17.0.12+7".into(),
            added_at: "t".into(),
            last_used_at: "t".into(),
        });
        assert_eq!(
            p.nearest_binding(Path::new("d:\\Work\\demo\\src"))
                .unwrap()
                .jdk,
            "temurin@17.0.12+7"
        );
        assert_eq!(
            p.nearest_binding(Path::new("D:/work/other")).unwrap().jdk,
            "temurin@21.0.4+7"
        );
        assert!(p.nearest_binding(Path::new("D:/elsewhere")).is_none());
        assert_eq!(p.count_bindings("temurin@17.0.12+7"), 1);
        assert!(p.remove_by_path(Path::new("D:/WORK/DEMO")));
        assert_eq!(p.projects.len(), 1);
    }

    #[test]
    fn path_key_normalization() {
        let a = path_key(Path::new("D:\\Work\\Demo"));
        let b = path_key(Path::new("d:/work/demo"));
        assert_eq!(a, b);
    }

    #[test]
    fn resolve_dir_absolute_vs_relative() {
        let home = Home::from_root("C:/x/.jvm");
        assert_eq!(
            resolve_dir(&home, "temurin_17"),
            PathBuf::from("C:/x/.jvm/versions/temurin_17")
        );
        assert_eq!(
            resolve_dir(&home, "D:/Dev-Software/jdk-17"),
            PathBuf::from("D:/Dev-Software/jdk-17")
        );
    }

    #[test]
    fn corrupted_files_fall_back_to_defaults() {
        let (_d, home) = tmp_home("corrupt");
        std::fs::write(home.index_file(), "{ not valid json").unwrap();
        std::fs::write(home.projects_file(), "]]]").unwrap();
        std::fs::write(home.settings_file(), "{\"language\":").unwrap();
        assert!(load_index(&home).unwrap().installed.is_empty());
        assert!(load_projects(&home).unwrap().projects.is_empty());
        assert_eq!(load_settings(&home).unwrap().language, "zh-CN");
    }

    #[test]
    fn settings_roundtrip_preserves_fields() {
        let (_d, home) = tmp_home("roundtrip");
        let s = Settings {
            language: "en".into(),
            proxy: "http://127.0.0.1:7890".into(),
            registry: "https://mirror.local/disco".into(),
            install_dir: "D:\\jdks".into(),
            download_dir: "D:\\dl".into(),
            source_api_key: "key-1".into(),
            registry_presets: vec![RegistryPreset {
                name: "镜像".into(),
                url: "https://m.local".into(),
                built_in: false,
            }],
            ..Settings::default()
        };
        save_settings(&home, &s).unwrap();
        let loaded = load_settings(&home).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn read_default_rejects_invalid_content() {
        let (_d, home) = tmp_home("bad-default");
        std::fs::write(home.default_file(), "not-an-id\n").unwrap();
        assert!(read_default(&home).is_err());
        // 空行/纯注释视为未设置
        std::fs::write(home.default_file(), "# comment\n\n").unwrap();
        assert!(read_default(&home).unwrap().is_none());
    }

    #[test]
    fn upsert_replaces_same_id() {
        let mut index = Index::default();
        let mk = |lts: bool| InstalledJdk {
            id: "temurin@17.0.12+7".into(),
            name: String::new(),
            vendor: "temurin".into(),
            version: "17.0.12+7".into(),
            feature: 17,
            lts,
            dir: String::new(),
            java_version_output: String::new(),
            installed_at: String::new(),
        };
        index.upsert(mk(false));
        index.upsert(mk(true));
        assert_eq!(index.installed.len(), 1, "同 id 记录应被替换");
        assert!(index.installed[0].lts);
    }

    #[test]
    fn installed_date_parses_rfc3339_to_local_date() {
        let rec = InstalledJdk {
            installed_at: "2026-10-01T20:30:00+00:00".into(),
            ..Default::default()
        };
        let date = rec.installed_date().unwrap();
        assert_eq!(date.len(), 10, "YYYY-MM-DD: {date}");
        assert_eq!(date.matches('-').count(), 2, "YYYY-MM-DD: {date}");
        // UTC 20:30 在东八区已是次日，说明确实做了时区转换
        let utc_date = "2026-10-01";
        let shifted = date != utc_date || chrono::Local::now().format("%z").to_string() == "+0000";
        assert!(
            shifted,
            "非零时区下 UTC 20:30 的本地日期应不同于 UTC 日期: {date}"
        );

        // 空/非法值 → None（旧索引、测试桩）
        assert!(InstalledJdk::default().installed_date().is_none());
        assert!(InstalledJdk {
            installed_at: "t".into(),
            ..Default::default()
        }
        .installed_date()
        .is_none());
    }

    #[test]
    fn jdk_id_uses_authoritative_id_field() {
        let internal = InstalledJdk {
            id: "temurin@17.0.12+7".into(),
            name: String::new(),
            vendor: "temurin".into(),
            version: "17.0.12+7".into(),
            feature: 17,
            lts: true,
            dir: String::new(),
            java_version_output: String::new(),
            installed_at: String::new(),
        };
        assert_eq!(internal.jdk_id().to_string(), "temurin@17.0.12+7");
    }
}
