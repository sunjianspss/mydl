//! RSS 订阅：定时抓取订阅源，按关键词规则挑出条目自动加进下载。
//!
//! 「已处理过的条目」必须持久化，否则每次重启都会把整个订阅源重下一遍 ——
//! 靠「是否已在任务列表里」判断是不够的，任务完成后会被删掉或移走。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use feed_rs::model::Entry;
use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::settings::{RssFeed, SettingsStore};

/// 每条订阅记住多少个已处理条目。订阅源一般只保留最近几十条，500 绰绰有余。
const SEEN_PER_FEED: usize = 500;

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 一次检查的结果，返回给界面显示。
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub feed: String,
    /// 订阅源里的条目总数。
    pub total: usize,
    /// 命中规则的条目数（含之前已处理过的）。
    pub matched: usize,
    /// 这次真正加进下载的数量。
    pub added: usize,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// 已处理条目的持久化
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct SeenData {
    /// feed id -> 已处理条目的标识，按时间先后排列。
    feeds: HashMap<String, VecDeque<String>>,
}

pub struct SeenStore {
    path: PathBuf,
    data: Mutex<SeenData>,
}

impl SeenStore {
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    fn contains(&self, feed_id: &str, item: &str) -> bool {
        self.data
            .lock()
            .unwrap()
            .feeds
            .get(feed_id)
            .is_some_and(|q| q.iter().any(|s| s == item))
    }

    fn remember(&self, feed_id: &str, item: String) {
        let mut data = self.data.lock().unwrap();
        let q = data.feeds.entry(feed_id.to_string()).or_default();
        q.push_back(item);
        while q.len() > SEEN_PER_FEED {
            q.pop_front();
        }
    }

    /// 写盘失败不影响下载，只会导致重启后可能重复添加一次。
    fn save(&self) {
        let data = self.data.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string(&*data) {
            Ok(json) => {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, &self.path);
                }
            }
            Err(e) => tracing::warn!("序列化 RSS 记录失败：{e:#}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 规则匹配
// ---------------------------------------------------------------------------

/// 标题是否符合规则：`include` 里的词要全中，`exclude` 里的一个都不能中。
///
/// 大小写不敏感。两个都为空时一律匹配。
pub fn matches(title: &str, include: &str, exclude: &str) -> bool {
    let hay = title.to_lowercase();

    if exclude
        .split_whitespace()
        .any(|w| hay.contains(&w.to_lowercase()))
    {
        return false;
    }
    include
        .split_whitespace()
        .all(|w| hay.contains(&w.to_lowercase()))
}

/// 从条目里找出能拿去下载的地址。
///
/// 优先级：磁力链 > 声明为 bittorrent 的 enclosure > 以 .torrent 结尾的链接。
/// 最后不兜底到普通网页链接 —— 那多半是详情页，加进去只会报错。
pub fn torrent_url(entry: &Entry) -> Option<String> {
    let media_urls = entry.media.iter().flat_map(|m| {
        m.content
            .iter()
            .filter_map(|c| c.url.as_ref().map(|u| (u.to_string(), c.content_type.as_ref().map(|t| t.to_string()))))
    });
    let link_urls = entry
        .links
        .iter()
        .map(|l| (l.href.clone(), l.media_type.clone()));
    let all: Vec<(String, Option<String>)> = media_urls.chain(link_urls).collect();

    all.iter()
        .find(|(u, _)| u.starts_with("magnet:"))
        .or_else(|| {
            all.iter().find(|(_, t)| {
                t.as_deref()
                    .is_some_and(|t| t.contains("bittorrent") || t.contains("x-torrent"))
            })
        })
        .or_else(|| {
            all.iter()
                .find(|(u, _)| u.split('?').next().unwrap_or(u).to_lowercase().ends_with(".torrent"))
        })
        .map(|(u, _)| u.clone())
}

/// 这条订阅实际用哪个目录：订阅自己指定的优先，否则用全局下载目录。
///
/// 空字符串当作没填 —— 界面上清空输入框留下的就是空串，直接传下去会变成
/// 「下载到当前工作目录」，那是个谁也想不到的地方。
pub fn effective_dir(feed_dir: Option<&str>, global: Option<String>) -> Option<String> {
    feed_dir
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .or(global)
}

/// 去重用的标识：优先用 guid，没有就退回下载地址。
fn item_key(entry: &Entry, url: &str) -> String {
    if entry.id.trim().is_empty() {
        url.to_string()
    } else {
        entry.id.clone()
    }
}

// ---------------------------------------------------------------------------
// 检查
// ---------------------------------------------------------------------------

pub async fn check_all(
    engine: &Arc<Engine>,
    store: &SettingsStore,
    seen: &SeenStore,
) -> Vec<CheckReport> {
    let settings = store.get();
    let client = match reqwest::Client::builder().timeout(FETCH_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            return vec![CheckReport {
                feed: "(全部)".into(),
                errors: vec![format!("创建 http client 失败：{e}")],
                ..Default::default()
            }]
        }
    };

    let mut reports = Vec::new();
    for feed in settings.rss_feeds.iter().filter(|f| f.enabled) {
        reports.push(check_one(&client, engine, feed, seen, settings.download_dir.clone()).await);
    }
    seen.save();
    reports
}

async fn check_one(
    client: &reqwest::Client,
    engine: &Arc<Engine>,
    feed: &RssFeed,
    seen: &SeenStore,
    download_dir: Option<String>,
) -> CheckReport {
    let label = if feed.name.trim().is_empty() {
        feed.url.clone()
    } else {
        feed.name.clone()
    };
    let mut report = CheckReport {
        feed: label.clone(),
        ..Default::default()
    };
    let dir = effective_dir(feed.dir.as_deref(), download_dir);

    let parsed = match fetch_and_parse(client, &feed.url).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(feed = %label, "抓取失败：{e:#}");
            report.errors.push(format!("{e:#}"));
            return report;
        }
    };

    report.total = parsed.entries.len();

    for entry in &parsed.entries {
        let title = entry
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_default();
        if !matches(&title, &feed.include, &feed.exclude) {
            continue;
        }
        report.matched += 1;

        let Some(url) = torrent_url(entry) else {
            tracing::debug!(feed = %label, %title, "命中规则但找不到下载地址");
            continue;
        };
        let key = item_key(entry, &url);
        if seen.contains(&feed.id, &key) {
            continue;
        }

        match engine.add(&url, dir.clone()).await {
            Ok(id) => {
                tracing::info!(feed = %label, %title, id, "RSS 自动添加");
                seen.remember(&feed.id, key);
                report.added += 1;
            }
            Err(e) => {
                tracing::warn!(feed = %label, %title, "添加失败：{e:#}");
                report.errors.push(format!("{title}：{e:#}"));
                // 不记进 seen，下次还会重试。
            }
        }
    }

    report
}

async fn fetch_and_parse(client: &reqwest::Client, url: &str) -> Result<feed_rs::model::Feed> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("请求 {url} 失败"))?
        .error_for_status()
        .with_context(|| format!("{url} 返回了错误状态"))?;
    let body = resp.bytes().await.context("读取订阅内容失败")?;
    feed_rs::parser::parse(&body[..]).context("解析订阅内容失败（不是合法的 RSS/Atom？）")
}

// ---------------------------------------------------------------------------
// 定时器
// ---------------------------------------------------------------------------

pub fn spawn(engine: Arc<Engine>, store: Arc<SettingsStore>, seen: Arc<SeenStore>) {
    tauri::async_runtime::spawn(async move {
        loop {
            // 每轮都重读间隔，改了设置不用重启。至少 5 分钟，免得手滑填 0
            // 把订阅源打爆。
            let minutes = store.get().rss_interval_minutes.max(5);
            tokio::time::sleep(Duration::from_secs(minutes * 60)).await;

            if store.get().rss_feeds.iter().all(|f| !f.enabled) {
                continue;
            }
            let reports = check_all(&engine, &store, &seen).await;
            let added: usize = reports.iter().map(|r| r.added).sum();
            if added > 0 {
                tracing::info!(added, "RSS 定时检查完成");
            }
        }
    });
}

pub fn seen_path(config_dir: &Path) -> PathBuf {
    config_dir.join("rss_seen.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_requires_all_keywords() {
        assert!(matches("Show S01E01 1080p WEB-DL", "show 1080p", ""));
        // 少一个词就不算命中
        assert!(!matches("Show S01E01 720p", "show 1080p", ""));
        // 空规则一律匹配
        assert!(matches("随便什么", "", ""));
    }

    #[test]
    fn exclude_wins_over_include() {
        assert!(!matches("Show 1080p HDTV", "show", "hdtv"));
        assert!(matches("Show 1080p WEB", "show", "hdtv"));
        // 多个排除词，命中任一即排除
        assert!(!matches("Show 720p", "", "480p 720p"));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(matches("SHOW 1080P", "show 1080p", ""));
        assert!(!matches("Show HDTV", "", "hdtv"));
    }

    #[test]
    fn feed_dir_falls_back_to_global() {
        let global = || Some("/全局".to_string());

        assert_eq!(
            effective_dir(Some("/剧集"), global()),
            Some("/剧集".to_string())
        );
        assert_eq!(effective_dir(None, global()), global());
        // 界面上清空输入框留下的是空串，不能当成「下载到当前工作目录」
        assert_eq!(effective_dir(Some("   "), global()), global());
        // 两个都没有就交给会话默认目录
        assert_eq!(effective_dir(None, None), None);
    }

    fn parse(xml: &str) -> Vec<Entry> {
        feed_rs::parser::parse(xml.as_bytes()).unwrap().entries
    }

    #[test]
    fn prefers_magnet_then_enclosure_then_dot_torrent() {
        let entries = parse(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>t</title>
            <item>
              <title>带磁力链</title>
              <guid>a</guid>
              <link>https://example.com/detail/1</link>
              <enclosure url="https://example.com/1.torrent" type="application/x-bittorrent" length="1"/>
            </item>
            <item>
              <title>只有 enclosure</title>
              <guid>b</guid>
              <link>https://example.com/detail/2</link>
              <enclosure url="https://example.com/2.torrent" type="application/x-bittorrent" length="1"/>
            </item>
            <item>
              <title>只有详情页</title>
              <guid>c</guid>
              <link>https://example.com/detail/3</link>
            </item>
            </channel></rss>"#,
        );

        // enclosure 声明了 bittorrent 类型，优先于普通 link
        assert_eq!(
            torrent_url(&entries[1]).as_deref(),
            Some("https://example.com/2.torrent")
        );
        // 只有网页链接时不该瞎猜 —— 那是详情页，加进去只会报错
        assert_eq!(torrent_url(&entries[2]), None);
        let _ = &entries[0];
    }

    #[test]
    fn finds_magnet_in_link() {
        let entries = parse(
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>t</title>
            <item><title>x</title><guid>g</guid>
            <link>magnet:?xt=urn:btih:abc&amp;tr=http://t</link></item>
            </channel></rss>"#,
        );
        assert!(torrent_url(&entries[0]).unwrap().starts_with("magnet:"));
    }

    /// 真抓一个公开订阅源，验证 HTTP + 解析这条链路。
    ///
    /// 这个源的 enclosure 是 video/h264，不是种子 —— 正好用来确认
    /// torrent_url 不会把普通媒体链接误当成种子（否则订阅一开就会
    /// 往下载列表里灌一堆垃圾）。
    #[tokio::test]
    #[ignore = "需要外网"]
    async fn fetches_and_parses_real_feed() {
        let client = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .build()
            .unwrap();
        let feed = fetch_and_parse(
            &client,
            "https://archive.org/services/collection-rss.php?collection=opensource_movies",
        )
        .await
        .expect("抓取或解析失败");

        assert!(!feed.entries.is_empty(), "订阅源该有条目");
        assert!(
            feed.entries.iter().any(|e| e.title.is_some()),
            "条目该有标题"
        );

        let bogus = feed.entries.iter().filter(|e| torrent_url(e).is_some()).count();
        assert_eq!(bogus, 0, "非种子源不该被认出任何下载地址，实际认出 {bogus} 个");
        eprintln!("OK: {} 条条目，0 个被误判为种子", feed.entries.len());
    }

    #[test]
    fn seen_store_survives_reload_and_caps_size() {
        let path = std::env::temp_dir().join(format!("mydl-rss-seen-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let store = SeenStore::load(path.clone());
        store.remember("feed1", "item1".into());
        store.save();

        assert!(SeenStore::load(path.clone()).contains("feed1", "item1"));
        assert!(!SeenStore::load(path.clone()).contains("feed1", "item2"));

        // 超过上限后最老的被丢掉，最新的保留。
        let store = SeenStore::load(path.clone());
        for i in 0..SEEN_PER_FEED + 10 {
            store.remember("feed2", format!("i{i}"));
        }
        assert!(!store.contains("feed2", "i0"));
        assert!(store.contains("feed2", &format!("i{}", SEEN_PER_FEED + 9)));

        let _ = std::fs::remove_file(&path);
    }
}
