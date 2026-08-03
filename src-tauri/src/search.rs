//! 搜索种子。走 Torznab —— Prowlarr 和 Jackett 说的是同一种协议，所以两个
//! 都能用，我们不关心背后是哪个。
//!
//! **结果里的 info-hash 全部来自索引器**，我们一个字节都不生成。这一点是
//! 整个搜索功能的地基：磁力链的 hash 是内容摘要，编不出来也猜不出来。
//! AI 排序（`ai.rs`）只允许对下面这个列表重新排序，不允许它产出链接。

use anyhow::{anyhow, bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

/// 一条搜索结果。
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub title: String,
    /// 磁力链。有些站只给 .torrent 地址，那时候这里是 None。
    pub magnet: Option<String>,
    /// .torrent 的下载地址，作为没有磁力链时的退路。
    pub link: Option<String>,
    pub size: u64,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
    /// 来自哪个站，Prowlarr 会在结果里带上。
    pub indexer: Option<String>,
    /// AI 排序时填的挑选理由；没开 AI 就是 None。
    pub reason: Option<String>,
    /// 这条是**模型从网上找来的**，不是索引器给的 —— 界面必须标出来。
    ///
    /// 索引器的 info-hash 来自站点数据库，可信；模型给的可能是从网页上抄的，
    /// 也可能是编的。两者混在一列里而不加区分是不诚实的。
    #[serde(default)]
    pub unverified: bool,
}

impl SearchResult {
    /// 能拿去添加的地址：优先磁力链，其次 .torrent。两个都没有就没法用。
    pub fn uri(&self) -> Option<&str> {
        self.magnet.as_deref().or(self.link.as_deref())
    }
}

/// 把 http(s) 地址解成真正能用的那个。
///
/// Jackett 的 `/dl/…` 端点对「只给磁力链的站」会回 **302 跳到 `magnet:`**
/// （它自己只是个跳板）。librqbit 拿到 302 直接报错 —— 通用 HTTP 客户端确实
/// 不该跟到 `magnet:` 这种非 HTTP 协议上去，所以得我们自己解一次。
///
/// 不是重定向、或者跳到另一个 http 地址（那多半是真的 .torrent 文件），
/// 就原样返回，交给 librqbit 自己下。解析失败也原样返回 —— 让后面真正的
/// 添加流程去报错，比在这里编一个错误信息强。
pub async fn resolve_uri(uri: &str) -> String {
    if !uri.starts_with("http://") && !uri.starts_with("https://") {
        return uri.to_string();
    }

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(c) => c,
        Err(_) => return uri.to_string(),
    };

    let Ok(resp) = client.get(uri).send().await else {
        return uri.to_string();
    };

    if !resp.status().is_redirection() {
        return uri.to_string();
    }

    let target = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    if target.starts_with("magnet:") {
        tracing::info!("索引器把下载地址重定向到了磁力链，改用磁力链添加");
        return target.to_string();
    }

    uri.to_string()
}

/// 结果标题是否和关键词相关。
///
/// **有些索引器匹配不到时会返回自己的默认榜单**（实测 The Pirate Bay 搜中文
/// 就这样，回 100 条当季新片），用户看到的就是「搜索坏了」。所以拿到结果后
/// 自己再筛一道。
///
/// 规则：**关键词全部命中才算**。按「非字母数字」切词 —— 空白、ASCII 标点、
/// 全角标点都算分隔符；单个拉丁字母的碎片忽略（`a`、`s` 这种匹配一切），
/// 中日韩单字有区分度所以保留 —— 「指环王」在英文标题里必然不命中，正好把
/// 无关结果筛掉。
///
/// 标点必须切，中文片名尤其吃这个亏：「碟中谍8：最终清算」不切就是**一个词**，
/// 标题里只要把全角冒号写成点（「碟中谍8.最终清算」）就再也匹配不上 —— 而各站
/// 的命名习惯本来就不统一。切开之后两段分别判断，既宽松够用又不会放进无关的。
///
/// （RSS 订阅的「包含」仍按空白分词：那边的词是用户一个个敲进去的，
/// 他写了什么就是什么，不该替他再切一刀。）
pub fn matches_query(title: &str, query: &str) -> bool {
    let title = title.to_lowercase();

    for token in query.split(|c: char| !c.is_alphanumeric()) {
        let token = token.to_lowercase();
        // 单字符的拉丁词没有区分度，跳过；中日韩单字有，所以只按字节长度判断
        // 会误伤，这里用「是不是纯 ASCII 且只有一个字符」来区分。
        if token.is_empty() || (token.len() == 1 && token.is_ascii()) {
            continue;
        }
        if !title.contains(&token) {
            return false;
        }
    }

    // 走到这里说明所有有意义的词都命中了；关键词全是噪音（比如只输了个
    // "a"）时一个都没检查过，那就不筛，交给索引器判断。
    true
}

/// 聚合地址（Jackett 的 `indexers/all`）要把所有索引器挨个问一遍才回，站点一多
/// 就很慢：12 个站实测 35~40 秒，比单独查一遍的总和还长。原来给 30 秒，结果是
/// 索引器明明正常返回了，却每次都被我们自己掐断。
const SEARCH_TIMEOUT_SECS: u64 = 60;

/// 请求索引器并解析结果。
///
/// `base` 是用户从 Prowlarr / Jackett 界面上复制的那条 Torznab 地址，
/// 里面已经带了 apikey —— 我们只往后面拼查询参数，不去猜各家的路径结构
/// （Prowlarr 和 Jackett 的前缀不一样，猜必错）。
pub async fn search(base: &str, query: &str) -> Result<Vec<SearchResult>> {
    let query = query.trim();
    if query.is_empty() {
        bail!("请输入搜索关键词");
    }
    let base = base.trim();
    if base.is_empty() {
        bail!("还没配置索引器地址。在设置里填 Prowlarr 或 Jackett 的 Torznab 地址");
    }

    let sep = if base.contains('?') { '&' } else { '?' };
    let url = format!(
        "{base}{sep}t=search&q={}",
        urlencoding_lite(query)
    );

    tracing::info!(query = %query, "搜索种子");

    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS))
        .send()
        .await
        // 超时和连不上得分开说。reqwest 把两者装在同一个错误里，混着报会
        // 把人支去查地址和进程 —— 而超时时地址通常是对的，只是索引器太慢。
        .map_err(|e| {
            if e.is_timeout() {
                anyhow!(
                    "索引器 {SEARCH_TIMEOUT_SECS} 秒没响应。\
                     索引器太多会拖慢聚合查询，可以在 Prowlarr / Jackett 里减掉几个（尤其是搜不出结果的），\
                     或者把地址换成单个索引器的"
                )
            } else {
                anyhow::Error::new(e).context("连不上索引器。检查地址对不对、服务在不在跑")
            }
        })?;

    let status = resp.status();
    let body = resp.text().await.context("读取索引器响应失败")?;

    if !status.is_success() {
        // Torznab 出错时会回一段 XML 说明原因，比光看状态码有用。
        let hint = extract_error(&body).unwrap_or_else(|| body.chars().take(200).collect());
        bail!("索引器返回 {status}：{hint}");
    }

    let all = parse(&body)?;
    let total = all.len();

    let kept: Vec<SearchResult> = all
        .into_iter()
        .filter(|r| matches_query(&r.title, query))
        .collect();

    if kept.len() != total {
        tracing::info!(
            总数 = total,
            保留 = kept.len(),
            "过滤掉与关键词无关的结果（有些索引器匹配不到时会返回默认榜单）"
        );
    }

    Ok(kept)
}

/// 只转义查询串里会破坏 URL 的字符。不引 urlencoding 依赖 —— 这里只处理
/// 一个查询参数，够用了。
fn urlencoding_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 解码 `GeneralRef` 事件的内容（`&` 和 `;` 之间的部分，如 `amp`、`#38`）。
///
/// quick-xml 把 `&amp;` 这类实体解析成独立的 `Event::GeneralRef`，不会并进
/// 相邻的 Text 事件，所以得自己把实体名还原成字符。未知实体保留 `&name;`
/// 原样，宁可显示原文也不猜。
fn decode_ref(content: &str) -> String {
    match content {
        "amp" => "&".to_string(),
        "lt" => "<".to_string(),
        "gt" => ">".to_string(),
        "quot" => "\"".to_string(),
        "apos" => "'".to_string(),
        // 十六进制要按 16 进制解析。用 parse::<u32>() 会把 "26" 当成十进制
        // 26（控制字符 U+001A），而 &#x26; 本该是 `&` —— 正是这个字符在
        // Jackett 下载地址里当参数分隔符，解错了链接照样是坏的。
        hex if hex.starts_with("#x") || hex.starts_with("#X") => {
            u32::from_str_radix(&hex[2..], 16)
                .ok()
                .and_then(char::from_u32)
                .map(|c| c.to_string())
                .unwrap_or_else(|| format!("&{hex};"))
        }
        dec if dec.starts_with('#') => dec[1..]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string())
            .unwrap_or_else(|| format!("&{dec};")),
        other => format!("&{other};"),
    }
}

/// Torznab 报错时的 `<error description="...">`。
fn extract_error(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"error" => {
                return attr(&e, b"description");
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.as_ref() == key).then(|| String::from_utf8_lossy(&a.value).into_owned())
    })
}

/// 解析 Torznab 的 RSS。
///
/// 用 quick-xml 而不是复用 feed-rs：种子数、体积这些关键字段都在
/// `<torznab:attr>` 自定义命名空间里，feed-rs 的模型不暴露它们。没有种子数
/// 就没法排序，而排序正是搜索的价值所在。
pub fn parse(xml: &str) -> Result<Vec<SearchResult>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut out = Vec::new();

    let mut cur: Option<SearchResult> = None;
    // 当前正在读哪个文本节点。Torznab 里 title/size/link 都是文本子节点。
    let mut field: Option<Vec<u8>> = None;
    // quick-xml 遇到 `&amp;` 这类实体会把文本拆成多个 Text 事件，这里累积起来，
    // 到元素结束时再拼成完整值 —— 直接按事件赋会给 link/title 留下半截（
    // Jackett 的下载地址 `…?apikey=…&amp;path=…` 就是这么被截断的）。
    let mut field_buf = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_vec();
                match name.as_slice() {
                    b"item" => cur = Some(SearchResult::default()),
                    b"title" | b"size" | b"link" | b"jackettindexer" => {
                        field = Some(name);
                        field_buf.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let Some(item) = cur.as_mut() else { continue };
                match e.name().as_ref() {
                    // <enclosure url="magnet:?..." length="123"/>
                    b"enclosure" => {
                        if let Some(u) = attr(&e, b"url") {
                            if u.starts_with("magnet:") {
                                item.magnet = Some(u);
                            } else if item.link.is_none() {
                                item.link = Some(u);
                            }
                        }
                        if item.size == 0 {
                            if let Some(l) = attr(&e, b"length").and_then(|v| v.parse().ok()) {
                                item.size = l;
                            }
                        }
                    }
                    // <torznab:attr name="seeders" value="12"/>
                    b"torznab:attr" | b"attr" => {
                        let (Some(k), Some(v)) = (attr(&e, b"name"), attr(&e, b"value")) else {
                            continue;
                        };
                        match k.as_str() {
                            "seeders" => item.seeders = v.parse().ok(),
                            "peers" | "leechers" => item.leechers = v.parse().ok(),
                            "magneturl" => item.magnet = Some(v),
                            "size" if item.size == 0 => item.size = v.parse().unwrap_or(0),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                // 只在关注字段内累积；quick-xml 会因实体分片，不能按事件直接赋值。
                if field.is_some() {
                    field_buf.push_str(&t.decode().unwrap_or_default());
                }
            }
            // `&amp;` 之类的实体是独立事件，不累积进去的话 link 里的 `&`
            // （Jackett 下载地址的参数分隔符）就会丢。
            Ok(Event::GeneralRef(r)) => {
                if field.is_some() {
                    field_buf.push_str(&decode_ref(&String::from_utf8_lossy(&r)));
                }
            }
            Ok(Event::End(e)) => {
                if e.name().as_ref() == b"item" {
                    if let Some(item) = cur.take() {
                        // 拿不到任何可用地址的结果留着也没意义。
                        if item.uri().is_some() && !item.title.is_empty() {
                            out.push(item);
                        }
                    }
                } else if let Some(f) = field.take() {
                    // 元素结束，把累积的完整文本提交给字段。
                    let text = field_buf.trim().to_string();
                    let Some(item) = cur.as_mut() else { continue };
                    match f.as_slice() {
                        b"title" => item.title = text,
                        b"size" if item.size == 0 => item.size = text.parse().unwrap_or(0),
                        b"link" => {
                            if text.starts_with("magnet:") {
                                item.magnet = Some(text);
                            } else if item.link.is_none() {
                                item.link = Some(text);
                            }
                        }
                        b"jackettindexer" => item.indexer = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => bail!("索引器返回的不是合法 XML：{e}"),
            _ => {}
        }
        buf.clear();
    }

    // 默认按种子数排，没有种子数的排最后 —— 这是没开 AI 时的兜底顺序。
    out.sort_by(|a, b| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一段真实形状的 Torznab 响应：既有 torznab:attr 里的 magneturl，
    /// 也有 enclosure 里的磁力链，还有一条只给 .torrent 的。
    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">
  <channel>
    <item>
      <title>Ubuntu 24.04 LTS Server amd64</title>
      <size>2937061376</size>
      <link>https://example.invalid/dl/abc.torrent</link>
      <torznab:attr name="seeders" value="120"/>
      <torznab:attr name="peers" value="8"/>
      <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"/>
    </item>
    <item>
      <title>Some Movie 2160p WEB-DL</title>
      <enclosure url="magnet:?xt=urn:btih:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" length="24696061952"/>
      <torznab:attr name="seeders" value="7"/>
    </item>
    <item>
      <title>Only Torrent File</title>
      <size>1048576</size>
      <link>https://example.invalid/dl/def.torrent</link>
    </item>
    <item>
      <title>No Usable Link</title>
      <size>100</size>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parses_torznab_and_sorts_by_seeders() {
        let r = parse(SAMPLE).unwrap();

        // 第四条没有任何可用地址，应该被丢掉。
        assert_eq!(r.len(), 3, "拿不到地址的结果不该留下");

        // 按种子数降序：120 → 7 → 无
        assert_eq!(r[0].seeders, Some(120));
        assert_eq!(r[1].seeders, Some(7));
        assert_eq!(r[2].seeders, None);

        // torznab:attr 里的 magneturl 优先于 <link> 里的 .torrent 地址
        assert!(r[0].magnet.as_deref().unwrap().starts_with("magnet:?xt=urn:btih:aaaa"));
        assert_eq!(r[0].link.as_deref(), Some("https://example.invalid/dl/abc.torrent"));
        assert_eq!(r[0].size, 2937061376);
        assert_eq!(r[0].leechers, Some(8));

        // enclosure 里的磁力链和体积也要认
        assert!(r[1].magnet.as_deref().unwrap().starts_with("magnet:?xt=urn:btih:bbbb"));
        assert_eq!(r[1].size, 24696061952);

        // 只有 .torrent 的那条，uri() 要退回到 link
        assert!(r[2].magnet.is_none());
        assert_eq!(r[2].uri(), Some("https://example.invalid/dl/def.torrent"));
    }

    /// Jackett 的下载地址在 Torznab 里会把 `&` 写成 `&amp;`（如
    /// `/dl/acgrip/?jackett_apikey=...&amp;path=...`）。quick-xml 遇到实体时
    /// 会把文本拆成多个 Text 事件，`is_none()` 这个 guard 会漏掉后半段。
    #[test]
    fn link_with_entity_is_not_truncated() {
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <item>
    <title>With Entity</title>
    <size>1048576</size>
    <link>http://127.0.0.1:9117/dl/acgrip/?jackett_apikey=abc&amp;path=XYZ</link>
  </item>
</channel></rss>"#;
        let r = parse(xml).unwrap();
        assert_eq!(
            r[0].link.as_deref(),
            Some("http://127.0.0.1:9117/dl/acgrip/?jackett_apikey=abc&path=XYZ"),
            "link 不该被 &amp; 截断"
        );
        assert_eq!(r[0].uri(), r[0].link.as_deref());
    }

    /// 三种实体写法都得解对。`&#x26;` 曾经被按十进制解析成 U+001A，
    /// 于是链接里的参数分隔符变成了控制字符，种子照样下不下来。
    #[test]
    fn decodes_all_entity_forms() {
        assert_eq!(decode_ref("amp"), "&");
        assert_eq!(decode_ref("#38"), "&", "十进制");
        assert_eq!(decode_ref("#x26"), "&", "十六进制小写");
        assert_eq!(decode_ref("#X26"), "&", "十六进制大写");
        assert_eq!(decode_ref("#x27"), "'");
        assert_eq!(decode_ref("lt"), "<");
        assert_eq!(decode_ref("gt"), ">");
        // 认不出来的原样保留，不猜
        assert_eq!(decode_ref("nbsp"), "&nbsp;");
        assert_eq!(decode_ref("#xZZ"), "&#xZZ;");
    }

    /// 走完整解析链路，确认十六进制实体不会把链接切坏。
    #[test]
    fn hex_entity_in_link_is_decoded() {
        let xml = r#"<rss><channel><item>
            <title>Hex Entity</title>
            <size>1024</size>
            <link>http://127.0.0.1:9117/dl/x/?apikey=abc&#x26;path=XYZ</link>
        </item></channel></rss>"#;
        let r = parse(xml).unwrap();
        assert_eq!(
            r[0].link.as_deref(),
            Some("http://127.0.0.1:9117/dl/x/?apikey=abc&path=XYZ")
        );
    }

    /// 实测 The Pirate Bay 搜「指环王」会返回 100 条当季新片。
    #[test]
    fn filters_irrelevant_results() {
        assert!(!matches_query("Disclosure.Day.2026.1080p.WEBRip", "指环王"));
        assert!(matches_query("指环王1.加长版.1080p", "指环王"));

        // 英文多词：全部命中才算
        assert!(matches_query(
            "The.Lord.of.the.Rings.The.Return.Of.The.King.2003",
            "Lord of the Rings"
        ));
        assert!(!matches_query("The.Hobbit.2012.1080p", "Lord of the Rings"));

        // 大小写无关
        assert!(matches_query("THE.MATRIX.1999", "matrix"));

        // 单个拉丁字母不参与判断，否则 "a" 会匹配一切
        assert!(matches_query("Anything At All", "a"));

        // 中日韩单字有区分度，要参与
        assert!(!matches_query("Some English Title", "龙"));
        assert!(matches_query("龙猫.1988", "龙"));
    }

    /// 中文片名里的全角标点要当分隔符。
    ///
    /// 实测：搜「碟中谍8：最终清算」时索引器回了 217 条，一条都没留下。整串当一
    /// 个词的话，标题只要换个分隔符就匹配不上，而各站的写法本来就五花八门。
    #[test]
    fn splits_on_cjk_punctuation() {
        let q = "碟中谍8：最终清算";

        // 分隔符跟关键词写得一样
        assert!(matches_query("梦幻天堂.BluRay.1080p.碟中谍8：最终清算.IMAX版", q));
        // 换成点、换成空格，一样该命中
        assert!(matches_query("碟中谍8.最终清算.2025.1080p.WEB-DL", q));
        assert!(matches_query("碟中谍8 最终清算 2025", q));

        // 但每一段都要在：上一部不算
        assert!(!matches_query("碟中谍7.致命清算.2023.1080p", q));
        // 默认榜单那种无关结果照样筛掉
        assert!(!matches_query("Disclosure.Day.2026.1080p.WEBRip", q));
    }

    #[test]
    fn reports_torznab_error() {
        let xml = r#"<error code="100" description="Invalid API Key"/>"#;
        assert_eq!(extract_error(xml).as_deref(), Some("Invalid API Key"));
    }

    #[test]
    fn encodes_query() {
        assert_eq!(urlencoding_lite("星球 大战"), "%E6%98%9F%E7%90%83+%E5%A4%A7%E6%88%98");
        assert_eq!(urlencoding_lite("a&b=c"), "a%26b%3Dc");
    }
}

