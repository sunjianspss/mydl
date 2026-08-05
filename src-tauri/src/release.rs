//! 从压制名里解析出「这是什么片子」。
//!
//! 用来给一个已有任务找替代源：任务名是压制名，不能直接拿去搜。
//!
//! # 为什么要拆成两个串
//!
//! `search::matches_query` 要求查询里**每个**词都是标题的子串。拿完整标题
//! 去搜，`碟中谍8：最终清算` 会被切成 `碟中谍8` + `最终清算` 两个词，而实际
//! 命名有的是 `碟中谍8`、有的是 `碟中谍：最终清算`，同时包含这两个连续子串
//! 的一条都没有 —— 实测 217 条结果全被滤掉。
//!
//! 所以分开：
//!
//! - `search_query` 只取核心标题（通常一个词），负责**召回**
//! - `full_title` 保留完整标题，负责给召回结果**排序**
//!
//! 宽召回 + 严排序，比一步到位的严格匹配稳得多。
//!
//! # 为什么不能只靠位置
//!
//! 英文 scene 命名是 `标题.年份.画质.来源.编码-组`，位置固定。中文压制不是：
//!
//! ```text
//! 梦幻天堂·龙网(www.321n.net).BluRay.1080p.碟中谍8：最终清算.IMAX版
//! ```
//!
//! 站点名在最前、画质在标题**前面**、片名在倒数第二段。所以这里靠的是
//! 「去掉所有认得出的噪音，剩下的 CJK 最长的那段就是片名」。

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    /// 拿去搜的串。尽量短，保召回。
    pub search_query: String,
    /// 完整标题，用来给搜索结果打分。
    pub full_title: String,
}

/// 画质 / 来源 / 编码 / 音轨等噪音词。全小写比对。
///
/// 只放**不可能是片名**的词。像 `ai` `bd` 这种两字母的不放，误伤风险高于收益。
const NOISE: &[&str] = &[
    // 分辨率
    "2160p", "1080p", "1080i", "720p", "576p", "480p", "4k", "8k", "uhd", "bd1080p", "bd720p",
    "hd1080p", "1920x1080", "3840x2160",
    // 来源
    "bluray", "blu-ray", "bdrip", "brrip", "remux", "web-dl", "webdl", "webrip", "web", "hdtv",
    "dvdrip", "dvd", "hdrip", "tvrip", "cam", "ts",
    // 编码
    "h264", "h265", "x264", "x265", "hevc", "avc", "av1", "xvid", "divx", "10bit", "8bit",
    "10bits",
    // 音轨
    "aac", "ac3", "eac3", "dts", "dts-hd", "dtshd", "truehd", "atmos", "ddp", "ddp5", "dd5",
    "ddp2", "flac", "mp3", "opus", "2audio", "2audios", "6audio",
    // 动态范围
    "hdr", "hdr10", "hdr10+", "dv", "dolby", "vision", "hlg", "sdr", "hq",
    // 版本
    "repack", "proper", "extended", "remastered", "uncut", "unrated", "imax", "criterion",
    "complete", "internal", "limited",
];

/// 中文的画质 / 字幕 / 版本标注。这些是子串匹配，不是整词匹配 ——
/// 它们经常直接粘在片名后面（`寒战1994[杜比视界版本]`）。
const CJK_NOISE: &[&str] = &[
    "杜比视界",
    "高码版",
    "双版本",
    "国语配音",
    "中文字幕",
    "简繁英字幕",
    "简繁字幕",
    "英字幕",
    "中英字幕",
    "内封字幕",
    "内嵌字幕",
    "简体",
    "繁体",
    "中字",
    "双语",
    "国粤双语",
    "版本",
    "收藏不迷路",
    "地址发布页",
    "最新电影",
    "更多无水印",
];

/// 域名后缀。命中就认为这一段是站点信息，不是片名。
const TLDS: &[&str] = &[
    ".com", ".net", ".org", ".cc", ".me", ".tv", ".club", ".xyz", ".top", ".info", ".biz", ".io",
];

fn looks_like_domain(s: &str) -> bool {
    let low = s.to_lowercase();
    low.contains("www.") || TLDS.iter().any(|t| low.contains(t))
}

fn is_year(s: &str) -> bool {
    s.len() == 4
        && s.bytes().all(|b| b.is_ascii_digit())
        && matches!(&s[..2], "19" | "20")
}

fn cjk_count(s: &str) -> usize {
    s.chars().filter(|c| is_cjk(*c)).count()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF     // 统一表意文字
        | 0x3400..=0x4DBF   // 扩展 A
        | 0x3040..=0x30FF   // 假名（日番也会碰到）
        | 0xAC00..=0xD7AF)  // 谚文
}

/// 去掉成对括号里的内容。
///
/// `【】` 基本都是站点头，`[]` 基本都是标注组，两种都整段扔掉。
/// 圆括号单独处理：里面装的常常是域名，而**紧挨在它前面的那段是站点名**
/// （`梦幻天堂·龙网(www.321n.net)`），要一起扔，否则站点名会被当成片名。
fn strip_brackets(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth_square = 0i32;
    let mut depth_full = 0i32;
    let mut paren = String::new();
    let mut in_paren = false;

    for c in name.chars() {
        match c {
            '【' | '［' => depth_full += 1,
            '】' | '］' => depth_full = (depth_full - 1).max(0),
            '[' => depth_square += 1,
            ']' => depth_square = (depth_square - 1).max(0),
            '(' | '（' if !in_paren => {
                in_paren = true;
                paren.clear();
            }
            ')' | '）' if in_paren => {
                in_paren = false;
                // 括号里是域名 -> 连同前面那段站点名一起丢掉。
                if looks_like_domain(&paren) {
                    truncate_trailing_segment(&mut out);
                } else if depth_square == 0 && depth_full == 0 {
                    out.push_str(&paren);
                }
                paren.clear();
            }
            _ => {
                if in_paren {
                    paren.push(c);
                } else if depth_square == 0 && depth_full == 0 {
                    out.push(c);
                }
            }
        }
    }
    // 括号没闭合就当普通文本收回来，别把后面的内容整段吃掉。
    if in_paren && !looks_like_domain(&paren) {
        out.push_str(&paren);
    }
    out
}

/// 把 `out` 末尾那一段（到上一个分隔符为止）删掉。用于丢弃域名前面的站点名。
fn truncate_trailing_segment(out: &mut String) {
    let cut = out
        .char_indices()
        .rev()
        .find(|(_, c)| matches!(c, '.' | ' ' | '_' | '】' | ']' | '-'))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    out.truncate(cut);
}

fn strip_cjk_noise(s: &str) -> String {
    let mut out = s.to_string();
    for n in CJK_NOISE {
        out = out.replace(n, " ");
    }
    out
}

/// 主入口。解析不出来时 `search_query` 会退回原名的前若干字符 ——
/// 宁可搜出一堆无关结果，也不要静悄悄地什么都不搜。
pub fn parse(name: &str) -> ParsedName {
    let cleaned = strip_cjk_noise(&strip_brackets(name));

    // 压制名以 `.` 分段为主，空格和下划线也当分隔符。
    let segments: Vec<&str> = cleaned
        .split(|c: char| c == '.' || c == '_' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let kept: Vec<&str> = segments
        .iter()
        .copied()
        .filter(|s| {
            let low = s.to_lowercase();
            !is_year(s)
                && !looks_like_domain(s)
                && !NOISE.contains(&low.as_str())
                // 发布组后缀：`DV-QuickIO`、`CHS-ENG-BTSJ6` 这种，最后一段带
                // 连字符且全是 ASCII。片名里不会长这样。
                && !(s.is_ascii() && s.contains('-') && s.len() > 3)
        })
        .collect();

    // 片名 = 剩下的段里 CJK 最多的那段。全英文压制则退回最长的那段。
    let title_seg = kept
        .iter()
        .copied()
        .max_by_key(|s| (cjk_count(s), s.chars().count()))
        .unwrap_or("");

    let full_title = title_seg.trim().to_string();

    // 核心标题：切到第一个中文冒号 / 破折号之前。
    // `碟中谍8：最终清算` -> `碟中谍8`，一个词，召回宽得多。
    let core = full_title
        .split(|c: char| matches!(c, '：' | ':' | '—' | '－'))
        .next()
        .unwrap_or("")
        .trim();

    let search_query = if core.is_empty() {
        // 什么都没解析出来：退回原名前 20 个字符，总比不搜强。
        name.chars().take(20).collect::<String>().trim().to_string()
    } else {
        core.to_string()
    };

    let full_title = if full_title.is_empty() {
        search_query.clone()
    } else {
        full_title
    };
    ParsedName {
        search_query,
        full_title,
    }
}

/// 候选和目标标题的贴合度，0.0~1.0。
///
/// 召回是宽的（只用核心标题搜），所以排序这一步必须严：按完整标题里的词
/// 在候选标题里命中了多少来算。命中率一样时由调用方拿做种数做次级排序。
pub fn relevance(candidate_title: &str, full_title: &str) -> f64 {
    let cand = candidate_title.to_lowercase();

    // CJK 按 2-gram 切，ASCII 按词切 —— 中文没有词边界，整段比对太苛刻。
    let mut grams: Vec<String> = Vec::new();
    for word in full_title.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        if cjk_count(word) > 0 {
            let chars: Vec<char> = word.chars().collect();
            if chars.len() == 1 {
                grams.push(chars[0].to_string());
            } else {
                for w in chars.windows(2) {
                    grams.push(w.iter().collect());
                }
            }
        } else if word.len() > 1 {
            grams.push(word.to_lowercase());
        }
    }

    if grams.is_empty() {
        return 0.0;
    }
    let hit = grams.iter().filter(|g| cand.contains(g.as_str())).count();
    hit as f64 / grams.len() as f64
}

/// 一个候选源。就是 `SearchResult` 加上两个判断结果。
#[derive(serde::Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub title: String,
    pub magnet: Option<String>,
    pub link: Option<String>,
    pub size: u64,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    pub indexer: Option<String>,
    /// 0~1，和当前任务标题的贴合度。
    pub relevance: f64,
    /// 这条就是你现在正在下的那个 —— 界面要标出来，不然会让人以为找到了新源。
    pub is_current: bool,
    /// 向公共 tracker 实查到的做种数。None = 没查到（磁力链认不出来、
    /// 或者 tracker 都没应答）。
    ///
    /// **优先显示这个而不是 `seeders`。** 实测某些中文索引器给所有条目
    /// 都填 `seeders=1, size=0.01GB` 这种占位值，照着它选源等于抛硬币。
    pub live_seeders: Option<u32>,
    pub live_leechers: Option<u32>,
}

/// 从磁力链里抠出 info-hash（小写）。认不出来返回 None。
pub fn info_hash_of(magnet: &str) -> Option<String> {
    let lower = magnet.to_lowercase();
    let start = lower.find("btih:")? + 5;
    let hex: String = lower[start..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    (hex.len() == 40).then_some(hex)
}

/// 相关度低于这个的不给看。召回是宽的，不筛的话两百多条全是噪音。
const RELEVANCE_FLOOR: f64 = 0.2;

/// 最多返回几条。
const MAX_CANDIDATES: usize = 20;

/// 给召回结果排序。
///
/// 排序键是「相关度优先，做种数次之」而不是反过来：一个做种数很高但根本不是
/// 这部片子的结果，排在前面比排在后面危险得多 —— 用户可能真的会点。
///
/// 筛完一条不剩时**退回按做种数给前十条**，而不是返回空列表：解析出来的标题
/// 可能就是错的，这时候让用户看到原始结果、自己改查询词，比告诉他「没找到」
/// 有用。
pub fn rank(
    results: Vec<crate::search::SearchResult>,
    full_title: &str,
    current_info_hash: &str,
) -> Vec<Candidate> {
    let current = current_info_hash.to_lowercase();

    let mut all: Vec<Candidate> = results
        .into_iter()
        .map(|r| {
            let is_current = r
                .magnet
                .as_deref()
                .and_then(info_hash_of)
                .is_some_and(|h| h == current);
            Candidate {
                relevance: relevance(&r.title, full_title),
                is_current,
                live_seeders: None,
                live_leechers: None,
                title: r.title,
                magnet: r.magnet,
                link: r.link,
                size: r.size,
                seeders: r.seeders,
                leechers: r.leechers,
                indexer: r.indexer,
            }
        })
        .collect();

    let sort = |v: &mut Vec<Candidate>| {
        v.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)))
        });
    };

    let mut kept: Vec<Candidate> = all
        .iter()
        .filter(|c| c.relevance >= RELEVANCE_FLOOR)
        .cloned()
        .collect();

    if kept.is_empty() {
        sort(&mut all);
        all.truncate(10);
        return all;
    }
    sort(&mut kept);
    kept.truncate(MAX_CANDIDATES);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchResult;

    fn result(title: &str, seeders: u32, magnet: Option<&str>) -> SearchResult {
        SearchResult {
            title: title.into(),
            magnet: magnet.map(Into::into),
            link: None,
            size: 1,
            seeders: Some(seeders),
            leechers: Some(0),
            indexer: None,
            reason: None,
            unverified: false,
        }
    }

    /// 这五个是仓库作者机器上的真实任务名，覆盖了中文压制命名的主要花样：
    /// 站点头在【】里、站点名带域名在圆括号里、画质在片名前面、
    /// 年份是片名的一部分、纯 ASCII 发布组后缀。
    #[test]
    fn parses_real_release_names() {
        let cases = [
            (
                "【高清影视之家发布 www.SSDSSE.com】星球大战：曼达洛人与古古[HDR+杜比视界双版本][简繁英字幕].2026.2160p.WEB-DL.DDP5.1.Atmos.H265.HDR.DV-ParkHD",
                "星球大战",
                "星球大战：曼达洛人与古古",
            ),
            (
                "梦幻天堂·龙网(www.321n.net).BluRay.1080p.碟中谍8：最终清算.IMAX版",
                "碟中谍8",
                "碟中谍8：最终清算",
            ),
            (
                "侏罗纪世界：重生.2025.BD1080P.AAC.H264.CHS-ENG.BTSJ6",
                "侏罗纪世界",
                "侏罗纪世界：重生",
            ),
        ];
        for (name, want_query, want_title) in cases {
            let got = parse(name);
            assert_eq!(got.search_query, want_query, "\n输入：{name}");
            assert_eq!(got.full_title, want_title, "\n输入：{name}");
        }
    }

    /// 年份陷阱：《寒战1994》的 1994 是**片名的一部分**，发行年是 2026。
    /// 按英文 scene 命名训练的解析器会把 1994 当年份摘掉，然后去找一部
    /// 1994 年的片子。
    #[test]
    fn keeps_year_that_belongs_to_the_title() {
        let got = parse(
            "【高清影视之家发布 www.HDBTHD.com】寒战1994[杜比视界版本][高码版][国语配音+中文字幕].Cold.War.1994.2026.2160p.HQ.WEB-DL.H265.DV.DTS-QuickIO",
        );
        assert_eq!(got.search_query, "寒战1994");
    }

    /// 站点名没在【】里，只有域名在圆括号里 —— 站点名必须跟着域名一起丢掉，
    /// 否则「梦幻天堂」会因为 CJK 更多而被当成片名。
    #[test]
    fn drops_site_name_attached_to_domain() {
        let got = parse("梦幻天堂·龙网(www.321n.net).BluRay.1080p.碟中谍8：最终清算.IMAX版");
        assert!(!got.search_query.contains("梦幻"), "实际：{got:?}");
        assert!(!got.search_query.contains("龙网"), "实际：{got:?}");
    }

    #[test]
    fn handles_pure_ascii_release() {
        let got = parse("Ubuntu.24.04.4.Live.Server.amd64");
        // 全英文时没有 CJK 可依靠，取最长的一段，够拿去搜。
        assert!(!got.search_query.is_empty());
        assert!(!got.search_query.contains("2160p"));
    }

    /// 一个字都解析不出来时不能返回空串 —— 那会让调用方去搜空查询。
    #[test]
    fn never_returns_empty_query() {
        for name in ["", "1080p.H265.DV", "...", "[][]"] {
            let got = parse(name);
            assert!(
                !got.search_query.is_empty() || name.trim().is_empty(),
                "输入 {name:?} 得到了空查询"
            );
        }
    }

    #[test]
    fn relevance_prefers_closer_titles() {
        let full = "碟中谍8：最终清算";
        let exact = relevance("碟中谍8：最终清算 2160p WEB-DL", full);
        let partial = relevance("碟中谍8 1080p", full);
        let wrong = relevance("速度与激情10 2160p", full);

        assert!(exact > partial, "完整命中该高于部分：{exact} vs {partial}");
        assert!(partial > wrong, "部分命中该高于无关：{partial} vs {wrong}");
        assert!(wrong < 0.2, "无关标题不该拿到 {wrong}");
    }

    /// 这条正是 `matches_query` 会误杀、而打分能救回来的情况：
    /// 候选只写了「碟中谍：最终清算」（没有 8），严格 AND 匹配直接淘汰，
    /// 但它其实高度相关。
    #[test]
    fn relevance_survives_missing_token() {
        let score = relevance("碟中谍：最终清算.2160p.WEB-DL", "碟中谍8：最终清算");
        assert!(score > 0.5, "该判为高度相关，实际 {score}");
    }
    #[test]
    fn extracts_info_hash_from_magnet() {
        let h = info_hash_of("magnet:?xt=urn:btih:A57B7F3548BCAB81116F5BD282DCE952B4215853&dn=x");
        assert_eq!(h.as_deref(), Some("a57b7f3548bcab81116f5bd282dce952b4215853"));
        // base32 的老式磁力链（32 字符）不该被当成 40 位十六进制。
        assert_eq!(info_hash_of("magnet:?xt=urn:btih:ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"), None);
        assert_eq!(info_hash_of("not a magnet"), None);
    }

    /// 当前正在下的那条必须被标出来，否则用户会以为找到了新源、白换一次。
    #[test]
    fn marks_the_torrent_you_already_have() {
        let cur = "a57b7f3548bcab81116f5bd282dce952b4215853";
        let out = rank(
            vec![
                result("碟中谍8：最终清算 2160p", 3, Some(&format!("magnet:?xt=urn:btih:{cur}"))),
                result("碟中谍8：最终清算 1080p", 40, Some("magnet:?xt=urn:btih:1111111111111111111111111111111111111111")),
            ],
            "碟中谍8：最终清算",
            cur,
        );
        let mine: Vec<_> = out.iter().filter(|c| c.is_current).collect();
        assert_eq!(mine.len(), 1);
        assert!(mine[0].title.contains("2160p"));
    }

    /// 相关度优先于做种数：一个做种数高但不相关的结果排在前面更危险 ——
    /// 用户真的会点。
    #[test]
    fn relevance_outranks_seeders() {
        let out = rank(
            vec![
                result("速度与激情10 2160p 中字", 900, None),
                result("碟中谍8：最终清算 1080p", 3, None),
            ],
            "碟中谍8：最终清算",
            "",
        );
        assert!(out[0].title.contains("碟中谍8"), "实际第一条：{}", out[0].title);
    }

    /// 这条是整个功能的兜底：解析出来的标题可能就是错的（中文压制命名太乱）。
    /// 那时候一条不剩地返回空列表，用户会以为"真没有源"，其实只是查询词不对。
    /// 必须退回按做种数给几条，让他能自己改词。
    #[test]
    fn falls_back_when_nothing_is_relevant() {
        let out = rank(
            vec![
                result("完全无关的东西 A", 10, None),
                result("完全无关的东西 B", 50, None),
            ],
            "碟中谍8：最终清算",
            "",
        );
        assert_eq!(out.len(), 2, "不该返回空列表");
        assert_eq!(out[0].seeders, Some(50), "退回时该按做种数排");
    }

}
