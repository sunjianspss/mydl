//! 拿真索引器验「给已有任务找替代源」这条链。
//!
//! 这个用例存在的理由不是回归，是**回答一个设计问题**：中文压制名解析出来的
//! 查询词，到底能不能从索引器捞回可用的候选？如果捞不回来，「自动换源」那套
//! 想法的前提就不成立。
//!
//! ```
//! MYDL_SEARCH_URL='http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab/api?apikey=...' \
//!   cargo test --test find_sources_live -- --ignored --nocapture
//! ```
//!
//! 不传 `MYDL_SEARCH_URL` 就跳过。

use mydl_lib::{health, release, search};

/// 作者机器上三个真实任务名，覆盖三种命名花样。
const NAMES: &[&str] = &[
    "梦幻天堂·龙网(www.321n.net).BluRay.1080p.碟中谍8：最终清算.IMAX版",
    "【高清影视之家发布 www.SSDSSE.com】星球大战：曼达洛人与古古[HDR+杜比视界双版本][简繁英字幕].2026.2160p.WEB-DL.DDP5.1.Atmos.H265.HDR.DV-ParkHD",
    "侏罗纪世界：重生.2025.BD1080P.AAC.H264.CHS-ENG.BTSJ6",
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要本地索引器（Prowlarr / Jackett）"]
async fn finds_alternatives_for_real_tasks() {
    let Ok(base) = std::env::var("MYDL_SEARCH_URL") else {
        eprintln!("没设 MYDL_SEARCH_URL，跳过");
        return;
    };

    let mut any_found = false;

    for name in NAMES {
        let parsed = release::parse(name);
        eprintln!("\n{}", "=".repeat(72));
        eprintln!("任务名  {name}");
        eprintln!("搜索词  {}", parsed.search_query);
        eprintln!("完整名  {}", parsed.full_title);

        let results = match search::search(&base, &parsed.search_query).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("搜索失败：{e:#}");
                continue;
            }
        };
        eprintln!("索引器返回 {} 条", results.len());

        let mut ranked = release::rank(results, &parsed.full_title, "");

        // 索引器的做种数不可信，拿 info-hash 去真 tracker 实查。
        let hashes: Vec<(String, [u8; 20])> = ranked
            .iter()
            .filter_map(|c| c.magnet.as_deref().and_then(release::info_hash_of))
            .filter_map(|h| health::parse_info_hash(&h).map(|raw| (h, raw)))
            .collect();
        let live = health::scrape_many(&hashes).await;
        for c in &mut ranked {
            if let Some(h) = c.magnet.as_deref().and_then(release::info_hash_of) {
                if let Some((s, l, ok)) = live.get(&h) {
                    if *ok > 0 {
                        c.live_seeders = Some(*s);
                        c.live_leechers = Some(*l);
                    }
                }
            }
        }

        eprintln!("排序后保留 {} 条（索引器做种数 / 实查做种数）：", ranked.len());
        for c in ranked.iter().take(8) {
            eprintln!(
                "   相关度 {:.2}  索引器 {:>4}  实查 {:>5}  {:>9}  {}",
                c.relevance,
                c.seeders.unwrap_or(0),
                c.live_seeders.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                format_size(c.size),
                c.title.chars().take(52).collect::<String>()
            );
        }
        if !ranked.is_empty() {
            any_found = true;
        }
    }

    assert!(
        any_found,
        "三个任务一条候选都没搜到 —— 「自动换源」的前提不成立，\
         这个结论本身就是有价值的，但先确认索引器配置是对的"
    );
}

fn format_size(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.2} GB", bytes as f64 / GB)
}
