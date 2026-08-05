//! 持久化设置。存成 JSON，放在 Tauri 的应用配置目录里。
//!
//! 以后 RSS 订阅规则、完成后自动化动作都往这个结构里加字段即可 ——
//! `#[serde(default)]` 保证旧的配置文件读得进来。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// 不派生 Eq：seed_ratio_limit 是 f64，而 f64 只有 PartialEq。
// 这里只需要能比较相等（测试里的 assert_eq!），够用。
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// 用户选定的下载目录。None 表示跟随系统默认下载文件夹。
    pub download_dir: Option<String>,

    /// 下载完成时发系统通知。
    pub notify_on_complete: bool,

    /// 下载完成时播一声提示音。
    ///
    /// 和通知分开：通知权限被拒、或者开了勿扰，系统通知就不会响，而这个
    /// 是我们自己播的，照样能听见。反过来也可以只要通知不要声音。
    pub sound_on_complete: bool,

    /// 完成后把内容移动到这个目录。None 表示不移动。
    ///
    /// 移动之后 librqbit 就找不到文件了，所以会顺带把任务从列表移除
    /// （文件保留）——也就是不再做种。
    pub move_to: Option<String>,

    /// 完成后解压内容里的 .zip。原压缩包保留。
    pub extract_archives: bool,

    /// 同时最多几个任务在下载。None = 不限。
    ///
    /// 超出的会被自动暂停，前面的下完再自动放出来。**只认这个进程自己暂停的
    /// 那些** —— 用户手动暂停的任务永远不会被自动恢复。代价是这份记录只在
    /// 内存里，重启后被自动暂停的任务需要手动继续。
    pub max_active_downloads: Option<usize>,

    /// 分享率到这个值就自动停止做种。None = 不限。
    ///
    /// **分享率每次重启会归零** —— librqbit 的 `uploaded_bytes` 只统计本次
    /// 会话，不持久化（会话文件里根本没这个字段）。所以这个值的实际含义是
    /// 「本次运行期间上传到几倍」，不是 PT 站看到的那个累计分享率。
    pub seed_ratio_limit: Option<f64>,

    /// 所有任务都完成后让电脑睡眠。默认关 —— 这是会打断你手头事情的动作。
    pub sleep_when_all_done: bool,

    /// RSS 订阅。
    pub rss_feeds: Vec<RssFeed>,

    /// 多久检查一次 RSS。
    pub rss_interval_minutes: u64,

    /// 给所有任务补充一组公共 tracker。
    ///
    /// 只有裸 info-hash 的磁力链没有自带 tracker，纯靠 DHT 找源。默认开着：
    /// 从搜索/剪贴板进来的磁力链绝大多数都是裸 hash，只靠 DHT 的话即使
    /// tracker 上明明有做种者也连不上，表现为任务挂着几天不动。
    /// 代价是你的 IP 会被上报给这些 tracker，所有任务都会 —— 介意就关掉。
    /// 改了要重启 App 才生效（会话创建时才读）。
    pub use_public_trackers: bool,

    /// 定期向公共 tracker scrape，记录每个任务的做种/下载人数走势。
    ///
    /// 单独一个开关而不是跟着 `use_public_trackers` 走：那个开关意味着
    /// 「向 tracker 汇报我在下载什么」，这个只是查询，暴露程度不同，但
    /// **同样会把 info-hash 发给这几个 tracker**，所以关掉时一个包都不发。
    pub swarm_health_check: bool,

    /// 多久采一次健康度。下限 10 分钟，见 `health::spawn`。
    pub swarm_health_interval_minutes: u64,

    /// BT 流量绑定到哪张网卡。三种取值：
    ///
    /// - `None`（默认）—— **自动挑一张物理网卡**，见 `netif::first_physical`
    /// - [`FOLLOW_SYSTEM_ROUTE`] —— 明确要求跟随系统默认路由，也就是
    ///   **有 VPN 时 BT 也走 VPN**。有人装 VPN 恰恰是为了让 BT 走它，
    ///   得留一条路让他说出来 —— 选某条 `utun*` 不等价，隧道重连后编号会变。
    /// - 具体接口名 —— 绑那一张
    ///
    /// 开着全局 VPN / 规则代理（TUN 模式）时默认路由指向 `utun*`，BT 也跟着
    /// 走隧道。后果是结构性的：隧道出口多半是机房 IP，会被大量 BT 客户端
    /// 屏蔽；UPnP 的多播出不了隧道，端口映射必然失败，**没有入站连接做种
    /// 就是无效劳动**。绑到物理网卡（macOS 走 IP_BOUND_IF）能绕过默认路由
    /// 直出，而**完全不动 VPN 本身**，浏览器照旧走隧道。
    ///
    /// 名字写错会让整个会话建不起来，所以界面上做成下拉，见 `netif.rs`。
    /// 改了要重启 App 才生效（会话创建时才读）。Windows 不支持。
    pub bind_device: Option<String>,

    /// 有任务在下载时阻止电脑休眠，下完自动解除。做种不算。
    pub prevent_sleep_while_downloading: bool,

    /// Prowlarr / Jackett 的 Torznab 地址（含 apikey），从它们界面上直接复制。
    ///
    /// 不拆成「地址 + key」两个字段：Prowlarr 和 Jackett 的路径前缀不一样，
    /// 我们去拼必错，不如让用户把整条粘过来。
    pub search_url: Option<String>,

    /// 大模型服务地址。默认 DeepSeek 的 OpenAI 兼容端点。
    pub ai_base_url: String,

    pub ai_model: String,

    /// 用模型给搜索结果排序。没填 key 时这个开关不起作用。
    pub ai_rank: bool,

    /// 切回窗口时看一眼剪贴板里有没有磁力链，有就提示添加。
    ///
    /// **只在窗口重新获得焦点时读一次**，不在后台轮询：常驻读剪贴板既让人
    /// 不安，macOS 15 起还会弹「某某读取了剪贴板」的系统提示。读到了也只是
    /// 显示一个横幅，绝不自动添加。
    pub watch_clipboard: bool,

    /// SOCKS5 代理，格式 `socks5://[用户名:密码@]主机:端口`。
    ///
    /// **只代理出站 TCP 连接。** DHT、uTP、UDP tracker 走的是 UDP，SOCKS5
    /// 代理不了，仍然是直连 —— 也就是说这不等于「BT 全程匿名」。改了要重启。
    pub proxy_url: Option<String>,

    /// IP 黑名单地址（如 iblocklist 那类列表）。会话启动时拉取。改了要重启。
    pub blocklist_url: Option<String>,

    /// 每个任务最多连多少 peer。弱网或老路由器上连接数太多会打爆 NAT 表。
    /// None = 用 librqbit 的默认值。改了要重启。
    pub peer_limit: Option<usize>,

    /// 全局下载限速，单位 KiB/s。None 或 0 表示不限。和上传限速一样运行时可改。
    pub download_limit_kbps: Option<u32>,

    /// 全局上传限速，单位 KiB/s。None 或 0 表示不限。
    ///
    /// 默认不限是因为限速会拖慢自己的下载（BT 靠上传换下载），但一旦上行
    /// 被打满，同一条线路上的其他人都会被牵连 —— 见 `Engine::set_upload_limit`。
    pub upload_limit_kbps: Option<u32>,
}

impl Settings {
    /// 换算成字节/秒给 Engine 用。0 和 None 一样当作不限速。
    pub fn upload_limit_bps(&self) -> Option<u32> {
        kbps_to_bps(self.upload_limit_kbps)
    }

    pub fn download_limit_bps(&self) -> Option<u32> {
        kbps_to_bps(self.download_limit_kbps)
    }
}

/// `bind_device` 的保留值：明确要求跟随系统默认路由。
///
/// 用尖括号是因为**真实网卡名里不可能有它** —— Linux 的 ifname 不允许空白和
/// `/`，实际命名也从来不用尖括号；macOS 的更是清一色 `en0` / `utun6` 这种。
/// 所以它和任何真接口名都不会撞。
pub const FOLLOW_SYSTEM_ROUTE: &str = "<system>";

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
            sound_on_complete: true,
            move_to: None,
            extract_archives: false,
            max_active_downloads: None,
            seed_ratio_limit: None,
            sleep_when_all_done: false,
            rss_feeds: Vec::new(),
            rss_interval_minutes: 30,
            use_public_trackers: true,
            swarm_health_check: true,
            swarm_health_interval_minutes: 30,
            bind_device: None,
            // 下载中不休眠是下载工具的常规行为，默认开。
            prevent_sleep_while_downloading: true,
            watch_clipboard: true,
            search_url: None,
            ai_base_url: "https://api.deepseek.com".into(),
            ai_model: "deepseek-v4-flash".into(),
            ai_rank: true,
            upload_limit_kbps: None,
            download_limit_kbps: None,
            proxy_url: None,
            blocklist_url: None,
            peer_limit: None,
        }
    }
}

fn kbps_to_bps(kbps: Option<u32>) -> Option<u32> {
    kbps.filter(|k| *k > 0).map(|k| k.saturating_mul(1024))
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
