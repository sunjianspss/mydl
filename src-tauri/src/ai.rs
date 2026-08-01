//! 用大模型给搜索结果排序。
//!
//! **模型只被允许返回序号，不允许返回链接。** 这不是提示词层面的约定，而是
//! 结构上的限制：[`rank`] 拿到序号后从**我们自己的**结果列表里取对象，模型
//! 输出里的任何其他内容都会被丢掉。
//!
//! 理由很硬：磁力链的 info-hash 是内容的 SHA-1 摘要，模型没法推导也没法记住。
//! 让它「给一条磁力链」，它只会编一串格式正确、DHT 里根本不存在的十六进制。
//! 排序不一样 —— 从标题里判断画质、字幕、压制组，正是它擅长的。

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::search::SearchResult;

/// 送给模型的候选条数上限。给太多既费 token 又没意义，用户也不会翻到第 40 条。
const MAX_CANDIDATES: usize = 25;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

pub struct AiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// 发一次 Responses 请求。
///
/// `web_search` 为 true 时带上 `tools: [{"type": "web_search"}]` —— 这是按
/// OpenAI Responses API 的惯例**推断**的参数名，官方文档只说了「原生支持
/// 联网搜索等工具调用」，没给出字段。所以带 tools 被拒时会**自动去掉 tools
/// 重试一次**：第三方代理是靠模型 id（如 deepseek-v4-flash-search）开搜索的，
/// 根本不需要这个参数，猜错了也不该让整个功能挂掉。
async fn ask(cfg: &AiConfig, instructions: &str, input: &str, web_search: bool) -> Result<String> {
    let url = format!("{}/responses", cfg.base_url.trim_end_matches('/'));

    let build = |with_tools: bool| {
        let mut b = serde_json::json!({
            "model": cfg.model,
            "instructions": instructions,
            "input": input,
        });
        if with_tools {
            b["tools"] = serde_json::json!([{ "type": "web_search" }]);
        }
        b
    };

    let mut with_tools = web_search;
    loop {
        let resp = reqwest::Client::new()
            .post(&url)
            .bearer_auth(&cfg.api_key)
            .json(&build(with_tools))
            .timeout(TIMEOUT)
            .send()
            .await
            .context("连不上模型服务")?;

        let status = resp.status();
        let text = resp.text().await.context("读取模型响应失败")?;

        if status.is_success() {
            let parsed: ResponsesReply =
                serde_json::from_str(&text).context("模型响应不是预期的 JSON")?;
            return Ok(parsed.text());
        }

        // 只对「带了 tools 才失败」这一种情况重试，且只重试一次。
        if with_tools && status.as_u16() == 400 {
            tracing::warn!("模型服务不认 tools 参数，改用不带 tools 重试（联网与否取决于模型 id）");
            with_tools = false;
            continue;
        }

        // 别把响应体整个抛出去，里面可能带着回显的请求内容。
        bail!(
            "模型服务返回 {status}：{}",
            text.chars().take(200).collect::<String>()
        );
    }
}

/// 没有索引器时的退路：让模型自己上网找。
///
/// **返回的每条都标了 `unverified`**，界面必须显示出来。模型给的链接可能是
/// 从网页上抄的真链接，也可能是编的 —— 我们无从分辨，只有加入任务时那次
/// 真实 DHT 探测能给出答案。
pub async fn search_web(cfg: &AiConfig, query: &str) -> Result<Vec<SearchResult>> {
    let input = format!(
        "帮我找这个资源的 BT 磁力链：{query}\n\n\
         只输出 JSON 数组，不要解释、不要围栏，最多 10 条：\n\
         [{{\"title\": \"完整发布名\", \"magnet\": \"magnet:?xt=urn:btih:...\", \"note\": \"来源或说明\"}}]\n\n\
         magnet 必须是你**真的在网页上看到**的完整磁力链，一个字符都不要改。\
         如果搜不到确切的磁力链，就返回空数组 []，**绝对不要凭印象拼一个** ——\
         info-hash 是内容摘要，编出来的必然无效，用户会白等两分钟超时。"
    );

    let content = ask(
        cfg,
        "你是一个资源检索助手。只输出 JSON，不要解释、不要围栏。",
        &input,
        true,
    )
    .await?;

    #[derive(Deserialize)]
    struct Found {
        title: String,
        magnet: String,
        #[serde(default)]
        note: String,
    }

    let found: Vec<Found> = serde_json::from_str(extract_json_array(&content))
        .context("模型没有按要求只输出 JSON 数组")?;

    Ok(found
        .into_iter()
        // 形状对不上的直接扔掉：至少得是 magnet:?xt=urn:btih: 加 40 位十六进制。
        .filter(|f| looks_like_magnet(&f.magnet))
        .map(|f| SearchResult {
            title: f.title,
            magnet: Some(f.magnet),
            reason: (!f.note.trim().is_empty()).then(|| f.note.trim().to_string()),
            unverified: true,
            ..Default::default()
        })
        .collect())
}

/// 只做形状检查。**这不能证明链接有效** —— 编造的 hash 一样是 40 位十六进制。
/// 真正的判定在 `Engine::preview` 那次 DHT 探测。
fn looks_like_magnet(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("magnet:?") else { return false };
    let Some(i) = rest.find("btih:") else { return false };
    rest[i + 5..]
        .chars()
        .take(40)
        .filter(|c| c.is_ascii_hexdigit())
        .count()
        == 40
}

/// Responses API 的回包。只挑我们要的那一段文本，其余字段一概不认 ——
/// 服务端加字段是常事，写死结构会平白失败。
#[derive(Deserialize)]
struct ResponsesReply {
    #[serde(default)]
    output: Vec<OutputItem>,
    /// 有些实现会直接给这个便捷字段。
    #[serde(default)]
    output_text: Option<String>,
}

#[derive(Deserialize)]
struct OutputItem {
    #[serde(default)]
    content: Vec<ContentItem>,
}

#[derive(Deserialize)]
struct ContentItem {
    #[serde(default)]
    text: Option<String>,
}

impl ResponsesReply {
    fn text(&self) -> String {
        if let Some(t) = &self.output_text {
            return t.clone();
        }
        self.output
            .iter()
            .flat_map(|o| o.content.iter())
            .filter_map(|c| c.text.as_deref())
            .collect::<Vec<_>>()
            .join("")
    }
}

/// 模型要返回的东西：只有序号和一句理由。
#[derive(Deserialize)]
struct Pick {
    index: usize,
    #[serde(default)]
    reason: String,
}

/// 按用户意图重排搜索结果，并给每条填上挑选理由。
///
/// 失败时返回原顺序而不是报错 —— 排序是锦上添花，不该因为模型抽风就让用户
/// 连搜索结果都看不到。
pub async fn rank(cfg: &AiConfig, query: &str, results: Vec<SearchResult>) -> Vec<SearchResult> {
    if results.len() < 2 {
        return results;
    }

    match rank_inner(cfg, query, &results).await {
        Ok(ordered) => ordered,
        Err(e) => {
            tracing::warn!("AI 排序失败，退回按种子数排序：{e:#}");
            results
        }
    }
}

async fn rank_inner(
    cfg: &AiConfig,
    query: &str,
    results: &[SearchResult],
) -> Result<Vec<SearchResult>> {
    let candidates: Vec<&SearchResult> = results.iter().take(MAX_CANDIDATES).collect();

    let listing = candidates
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "{i}. {} | {:.2} GB | 种子 {} | 下载者 {}",
                r.title,
                r.size as f64 / 1024.0 / 1024.0 / 1024.0,
                r.seeders.map_or("未知".into(), |s| s.to_string()),
                r.leechers.map_or("未知".into(), |s| s.to_string()),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "用户在找：{query}\n\n\
         下面是索引器返回的候选，每行开头是序号：\n{listing}\n\n\
         按「最符合用户意图」重新排序。判断依据：分辨率和画质标记、是否有中文字幕、\
         压制组口碑、体积是否合理（同画质下过小多半是压得太狠）、种子数是否够多。\
         明显是枪版、样片、广告或体积异常的排到最后。\n\n\
         只输出 JSON 数组，不要任何其他文字，格式：\n\
         [{{\"index\": 3, \"reason\": \"一句话说明为什么排这里\"}}]\n\n\
         index 必须是上面出现过的序号。理由用中文，不超过 20 字。"
    );

    // 排序不需要联网 —— 候选就在提示词里。
    let content = ask(
        cfg,
        "你是一个 BT 资源筛选助手。只输出 JSON，不要解释、不要围栏。",
        &prompt,
        false,
    )
    .await?;

    let picks: Vec<Pick> = serde_json::from_str(extract_json_array(&content))
        .context("模型没有按要求只输出 JSON 数组")?;

    // 关键的一步：**只按序号取我们自己的对象**。模型输出里如果夹带了链接、
    // 标题或者别的什么，到这里全部被丢掉；越界的序号也直接忽略。
    let mut seen = vec![false; candidates.len()];
    let mut ordered: Vec<SearchResult> = Vec::with_capacity(results.len());
    for p in picks {
        if p.index >= candidates.len() || seen[p.index] {
            continue;
        }
        seen[p.index] = true;
        let mut r = candidates[p.index].clone();
        r.reason = (!p.reason.trim().is_empty()).then(|| p.reason.trim().to_string());
        ordered.push(r);
    }

    // 模型漏掉的候选按原顺序补在后面，一条都不丢。
    for (i, c) in candidates.iter().enumerate() {
        if !seen[i] {
            ordered.push((*c).clone());
        }
    }
    // 超出 MAX_CANDIDATES 的那些也照样保留。
    ordered.extend(results.iter().skip(candidates.len()).cloned());

    Ok(ordered)
}

/// 模型爱在 JSON 外面套 ```json 围栏或者加一句客套话，把中间那段抠出来。
fn extract_json_array(s: &str) -> &str {
    let start = s.find('[');
    let end = s.rfind(']');
    match (start, end) {
        (Some(a), Some(b)) if b > a => &s[a..=b],
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results() -> Vec<SearchResult> {
        (0..3)
            .map(|i| SearchResult {
                title: format!("片子 {i}"),
                magnet: Some(format!("magnet:?xt=urn:btih:{:040x}", i)),
                size: 1000,
                ..Default::default()
            })
            .collect()
    }

    /// 模型即使回了一条伪造的磁力链，也绝不可能进到结果里 —— 我们只认序号。
    #[test]
    fn model_output_cannot_inject_links() {
        let content = r#"这是我的排序：
```json
[{"index": 2, "reason": "画质最好", "magnet": "magnet:?xt=urn:btih:deadbeef"},
 {"index": 0, "reason": "备选"}]
```"#;
        let picks: Vec<Pick> = serde_json::from_str(extract_json_array(content)).unwrap();
        let src = results();

        let mut seen = vec![false; src.len()];
        let mut ordered = Vec::new();
        for p in picks {
            if p.index >= src.len() || seen[p.index] {
                continue;
            }
            seen[p.index] = true;
            let mut r = src[p.index].clone();
            r.reason = Some(p.reason);
            ordered.push(r);
        }

        assert_eq!(ordered[0].title, "片子 2");
        assert_eq!(ordered[0].reason.as_deref(), Some("画质最好"));
        // 伪造的那条磁力链没有任何机会出现在结果里
        assert_eq!(ordered[0].magnet.as_deref(), src[2].magnet.as_deref());
        assert!(!ordered.iter().any(|r| r.magnet.as_deref() == Some("magnet:?xt=urn:btih:deadbeef")));
    }

    /// Responses API 的回包形状：既认便捷字段，也认 output 数组。
    #[test]
    fn reads_both_response_shapes() {
        let a: ResponsesReply =
            serde_json::from_str(r#"{"output_text":"[{\"index\":1}]"}"#).unwrap();
        assert_eq!(a.text(), "[{\"index\":1}]");

        let b: ResponsesReply = serde_json::from_str(
            r#"{"id":"x","output":[{"type":"message","content":[{"type":"output_text","text":"[{\"index\":2}]"}]}]}"#,
        )
        .unwrap();
        assert_eq!(b.text(), "[{\"index\":2}]");
    }

    #[test]
    fn rejects_malformed_magnets() {
        assert!(looks_like_magnet(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x"
        ));
        // 位数不够
        assert!(!looks_like_magnet("magnet:?xt=urn:btih:deadbeef"));
        // 根本不是磁力链
        assert!(!looks_like_magnet("https://example.invalid/a.torrent"));
        assert!(!looks_like_magnet("随便一句话"));
    }

    #[test]
    fn strips_code_fence_and_prose() {
        assert_eq!(extract_json_array("好的：\n```json\n[{\"index\":1}]\n```\n"), "[{\"index\":1}]");
        assert_eq!(extract_json_array("[{\"index\":1}]"), "[{\"index\":1}]");
    }
}
