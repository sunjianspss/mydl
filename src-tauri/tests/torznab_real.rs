//! 用真实的 Jackett 响应验一遍解析器。
//!
//! 样本由本机 Jackett 抓下来后放进 tests/fixtures/，不联网也能跑 ——
//! 之前只用手写的小样本验过，字段齐全但形状太干净。
//!
//! 样本里的 apikey 已替换成 REDACTED_APIKEY，截到 40 条。原始响应有 159 条、
//! 548KB，且**每条链接里都带着真实的 apikey** —— 抓 fixture 时务必先脱敏。

use mydl_lib::search::parse;

#[test]
fn parses_real_jackett_response() {
    let xml = include_str!("fixtures/jackett_lotr.xml");
    let r = parse(xml).expect("真实响应应该解析成功");

    assert!(r.len() >= 30, "只解析出 {} 条，太少了", r.len());
    assert!(r.iter().all(|x| !x.title.is_empty()), "有条目没标题");
    assert!(r.iter().all(|x| x.uri().is_some()), "有条目没有可用地址");

    let with_seeders = r.iter().filter(|x| x.seeders.is_some()).count();
    assert!(with_seeders > r.len() / 2, "种子数解析出来的太少：{with_seeders}/{}", r.len());

    let with_magnet = r.iter().filter(|x| x.magnet.is_some()).count();
    assert!(with_magnet > 0, "一条磁力链都没解析出来");

    // 默认按种子数降序
    let seeds: Vec<u32> = r.iter().map(|x| x.seeders.unwrap_or(0)).collect();
    assert!(seeds.windows(2).all(|w| w[0] >= w[1]), "没有按种子数降序");

    eprintln!(
        "OK: {} 条，{} 条有种子数，{} 条有磁力链，最高 {} 个种子",
        r.len(), with_seeders, with_magnet, seeds[0]
    );
}

/// Jackett 的 .torrent 下载地址形如 `…/dl/acgrip/?apikey=…&amp;path=…&amp;file=…`，
/// `&` 在 XML 里是 `&amp;`，解析时不能丢（丢了 path 就下不了种子文件）。
#[test]
fn dl_links_keep_path_param() {
    let xml = include_str!("fixtures/jackett_lotr.xml");
    let r = parse(xml).expect("真实响应应该解析成功");

    let dl: Vec<&str> = r
        .iter()
        .filter_map(|x| x.link.as_deref().filter(|l| l.contains("/dl/")))
        .collect();
    assert!(!dl.is_empty(), "fixture 里该有 Jackett 的下载链接");
    for l in &dl {
        assert!(l.contains("&path="), "dl 链接缺 &path=：{l}");
    }
}
