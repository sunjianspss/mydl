//! 持久化设置。存成 JSON，放在 Tauri 的应用配置目录里。
//!
//! 以后 RSS 订阅规则、完成后自动化动作都往这个结构里加字段即可 ——
//! `#[serde(default)]` 保证旧的配置文件读得进来。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// 用户选定的下载目录。None 表示跟随系统默认下载文件夹。
    pub download_dir: Option<String>,

    /// 下载完成时发系统通知。
    pub notify_on_complete: bool,

    /// 完成后把内容移动到这个目录。None 表示不移动。
    ///
    /// 移动之后 librqbit 就找不到文件了，所以会顺带把任务从列表移除
    /// （文件保留）——也就是不再做种。
    pub move_to: Option<String>,

    /// 完成后解压内容里的 .zip。原压缩包保留。
    pub extract_archives: bool,

    /// RSS 订阅。
    pub rss_feeds: Vec<RssFeed>,

    /// 多久检查一次 RSS。
    pub rss_interval_minutes: u64,

    /// 给所有任务补充一组公共 tracker。
    ///
    /// 只有裸 info-hash 的磁力链没有自带 tracker，纯靠 DHT 找源；补上公共
    /// tracker 能多一条路。代价是你的 IP 会被上报给这些 tracker，所有任务
    /// 都会 —— 所以默认关闭。改了要重启 App 才生效（会话创建时才读）。
    pub use_public_trackers: bool,

    /// 有任务在下载时阻止电脑休眠，下完自动解除。做种不算。
    pub prevent_sleep_while_downloading: bool,

    /// 全局上传限速，单位 KiB/s。None 或 0 表示不限。
    ///
    /// 默认不限是因为限速会拖慢自己的下载（BT 靠上传换下载），但一旦上行
    /// 被打满，同一条线路上的其他人都会被牵连 —— 见 `Engine::set_upload_limit`。
    pub upload_limit_kbps: Option<u32>,
}

impl Settings {
    /// 换算成字节/秒给 Engine 用。0 和 None 一样当作不限速。
    pub fn upload_limit_bps(&self) -> Option<u32> {
        self.upload_limit_kbps
            .filter(|k| *k > 0)
            .map(|k| k.saturating_mul(1024))
    }
}

/// 几个长期在运行的开放 tracker。开启后对所有任务生效。
pub const PUBLIC_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.tracker.cl:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.torrent.eu.org:451/announce",
];

/// 一条 RSS 订阅及其过滤规则。
///
/// 过滤用空格分隔的关键词而不是正则：关键词写错了顶多不匹配，正则写错了
/// 可能匹配到一切，对一个会自动开始下载的功能来说前者安全得多。
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct RssFeed {
    /// 稳定标识，用来记「这条订阅里哪些条目已经处理过」。
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    /// 全部命中才算匹配。空 = 不过滤。
    pub include: String,
    /// 命中任一即排除。
    pub exclude: String,
}

impl Default for RssFeed {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            enabled: true,
            include: String::new(),
            exclude: String::new(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: None,
            // 通知是无害的，默认开；会动文件的两项默认关。
            notify_on_complete: true,
            move_to: None,
            extract_archives: false,
            rss_feeds: Vec::new(),
            rss_interval_minutes: 30,
            use_public_trackers: false,
            // 下载中不休眠是下载工具的常规行为，默认开。
            prevent_sleep_while_downloading: true,
            upload_limit_kbps: None,
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    /// 读不出来就用默认值继续，绝不因为配置文件坏了就起不来。
    pub fn load(path: PathBuf) -> Self {
        let current = match read_settings(&path) {
            Ok(s) => s,
            Err(e) => {
                // 文件不存在是正常情况（首次启动），不值得报警。
                if path.exists() {
                    tracing::warn!("读取设置失败，改用默认值：{e:#}");
                }
                Settings::default()
            }
        };

        Self {
            path,
            current: Mutex::new(current),
        }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap().clone()
    }

    pub fn set_download_dir(&self, dir: Option<String>) -> Result<()> {
        let mut guard = self.current.lock().unwrap();
        guard.download_dir = dir;
        write_settings(&self.path, &guard)
    }

    /// 整份替换。下载目录只在启动时读，所以这里不允许改它 ——
    /// 改了界面会显示新目录、实际却还写在旧目录，比不给改更糟。
    pub fn update(&self, mut next: Settings) -> Result<()> {
        let mut guard = self.current.lock().unwrap();
        next.download_dir = guard.download_dir.clone();
        *guard = next;
        write_settings(&self.path, &guard)
    }
}

fn read_settings(path: &Path) -> Result<Settings> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("无法读取 {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("{} 不是合法的设置文件", path.display()))
}

/// 先写临时文件再 rename，避免写到一半崩了留下半个损坏的配置。
fn write_settings(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建配置目录 {}", parent.display()))?;
    }

    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(settings).context("序列化设置失败")?;
    std::fs::write(&tmp, json).with_context(|| format!("无法写入 {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("无法替换 {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mydl-settings-{}-{tag}.json",
            std::process::id()
        ))
    }

    #[test]
    fn missing_file_yields_defaults() {
        let p = tmp_path("missing");
        let _ = std::fs::remove_file(&p);
        assert_eq!(SettingsStore::load(p).get(), Settings::default());
    }

    #[test]
    fn survives_restart() {
        let p = tmp_path("roundtrip");
        let _ = std::fs::remove_file(&p);

        let store = SettingsStore::load(p.clone());
        store
            .set_download_dir(Some("/tmp/我的下载".into()))
            .unwrap();

        // 模拟重启：重新从磁盘读。
        let reloaded = SettingsStore::load(p.clone());
        assert_eq!(reloaded.get().download_dir.as_deref(), Some("/tmp/我的下载"));

        // 恢复默认也要落盘。
        reloaded.set_download_dir(None).unwrap();
        assert_eq!(SettingsStore::load(p.clone()).get().download_dir, None);

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let p = tmp_path("corrupt");
        std::fs::write(&p, b"{ this is not json").unwrap();

        // 关键：坏配置不能让 App 起不来。
        assert_eq!(SettingsStore::load(p.clone()).get(), Settings::default());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        // 新版本写了旧版本不认识的字段时，旧版本不该直接崩。
        let p = tmp_path("future");
        std::fs::write(
            &p,
            br#"{"downloadDir":"/tmp/x","someFutureFeature":{"a":1}}"#,
        )
        .unwrap();

        assert_eq!(
            SettingsStore::load(p.clone()).get().download_dir.as_deref(),
            Some("/tmp/x")
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_fields_use_defaults() {
        // 老版本写的配置文件没有新字段，读进来该用默认值而不是失败。
        let p = tmp_path("old");
        std::fs::write(&p, br#"{"downloadDir":"/tmp/x"}"#).unwrap();

        let s = SettingsStore::load(p.clone()).get();
        assert_eq!(s.download_dir.as_deref(), Some("/tmp/x"));
        assert!(s.notify_on_complete, "通知默认该是开的");
        assert!(s.rss_feeds.is_empty());
        assert_eq!(s.rss_interval_minutes, 30);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn malformed_known_field_resets_everything() {
        // 已知字段类型不对会导致整份解析失败，于是所有设置都回到默认值 ——
        // 包括下载目录。这是有意的取舍（宁可回默认也不要半份配置），
        // 但值得用测试把这个行为钉住。
        let p = tmp_path("malformed");
        // rssFeeds 该是对象数组，这里给字符串数组。
        std::fs::write(&p, br#"{"downloadDir":"/tmp/x","rssFeeds":["oops"]}"#).unwrap();

        assert_eq!(SettingsStore::load(p.clone()).get(), Settings::default());
        let _ = std::fs::remove_file(&p);
    }
}
