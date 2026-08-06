//! 验伪：下之前先确认这个文件**真的是名字说的那个东西**。
//!
//! # BT 的一个被浪费的优势
//!
//! BT 可以**按需取任意 piece**。所以不必等 14 GB 下完才发现被骗 —— 取头部
//! 那几 MB，从容器元数据里读出真实分辨率、时长和音轨，跟名字里吹的对一下
//! 就行。一帧都不用解码，见 [`crate::media`]。
//!
//! # 判什么
//!
//! | 检查 | 为什么可靠 |
//! |---|---|
//! | 分辨率虚标 | 名字写 2160p、实际 1080p 上采样。容器里的像素数不会撒谎 |
//! | 音轨对不上 | 名字写「国语配音」但一条中文轨都没有 |
//! | 根本不是视频 | 容器认不出来 —— 可能是 exe，也可能只是罕见格式 |
//! | 码率离谱 | 体积 ÷ 时长。2160p 只有 0.5 Mbps 必然是垃圾 |
//!
//! # 判不了什么
//!
//! **不判「是不是这部片子」。** 那需要外部数据（TMDB 之类）和内容指纹，
//! 而这里只有容器头。宁可少说也不诬告。
//!
//! 时长也**不单独判**：我们不知道用户下的是电影、剧集还是纪录片，
//! 「90 分钟才正常」这种假设错得起。时长只参与码率计算。

use serde::Serialize;

use crate::media::MediaInfo;

/// 名字里吹的。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Claimed {
    /// 声称的高度，从 `2160p` / `1080p` / `4K` 这类标记里认出来。
    pub height: Option<u32>,
    /// 名字里出现了「国语」「国配」「中字」这类中文相关标记。
    pub chinese_audio: bool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// 对得上。
    Ok,
    /// 有出入，但可能有正当理由。
    Warn,
    /// 明确对不上。
    Bad,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub level: Level,
    pub text: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    /// 实测到的规格，给界面直接显示。
    pub container: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_secs: Option<f64>,
    pub audio_langs: Vec<String>,
    pub bitrate_mbps: Option<f64>,
    pub findings: Vec<Finding>,
    /// 一句话总结。
    pub verdict: String,
}

/// 中文音轨可能的标记。
///
/// **两套编码都要认**：Matroska 的 `Language` 用 ISO-639-2（`chi`/`zho`），
/// 而 `LanguageBCP47` 用两字母（`zh`）。实测手上 9 个真实文件**全都是两字母
/// 的 BCP47** —— 只认三字母的话，「有没有国配」永远判成没有。
const CHINESE: &[&str] = &["zh", "chi", "zho", "cmn", "yue", "zh-cn", "zh-hans", "zh-hant"];

fn is_chinese(lang: &str) -> bool {
    let l = lang.to_ascii_lowercase();
    CHINESE.iter().any(|c| l == *c || l.starts_with("zh-"))
}

/// 从压制名里认出「声称的规格」。
pub fn claimed_from_name(name: &str) -> Claimed {
    let lower = name.to_ascii_lowercase();

    // 注意顺序：先长后短，否则 "1080p" 会被 "80p" 之类误伤。
    let height = [
        ("4320p", 4320u32), ("2160p", 2160), ("1440p", 1440), ("1080p", 1080),
        ("1080i", 1080), ("720p", 720), ("576p", 576), ("480p", 480),
        ("8k", 4320), ("4k", 2160), ("uhd", 2160),
    ]
    .iter()
    .find(|(tag, _)| lower.contains(tag))
    .map(|(_, h)| *h);

    const CN_AUDIO: &[&str] = &["国语", "国配", "普通话", "中文配音", "国英", "粤语", "双语", "国粤"];
    let chinese_audio = CN_AUDIO.iter().any(|t| name.contains(t));

    Claimed {
        height,
        chinese_audio,
    }
}

/// 实际高度低于声称高度多少才算虚标。
///
/// 不要求严格相等：宽银幕片子会裁掉黑边，`2160p` 实际是 `3840x1608`，
/// `1080p` 是 `1920x800` —— 实测手上 9 个文件里有 4 个这样。按**宽度**判
/// 才对得上，但有些片源又是按高度对齐的，所以两边都给足容差。
const HEIGHT_TOLERANCE: f64 = 0.75;

/// 判读。纯函数。
pub fn judge(info: &MediaInfo, claimed: &Claimed, file_size: u64) -> VerifyReport {
    let mut findings = Vec::new();

    let bitrate_mbps = match info.duration_secs {
        Some(d) if d > 1.0 => Some(file_size as f64 * 8.0 / d / 1e6),
        _ => None,
    };

    // 1. 分辨率
    match (claimed.height, info.height, info.width) {
        (Some(want), Some(got), Some(got_w)) => {
            // 宽银幕会裁高度，所以拿「宽度推算出的等效高度」一起看：
            // 2160p 的宽是 3840，1080p 是 1920。
            let effective = got.max((got_w as f64 / (16.0 / 9.0)) as u32);
            if (effective as f64) < want as f64 * HEIGHT_TOLERANCE {
                findings.push(Finding {
                    level: Level::Bad,
                    text: format!(
                        "名字写 {want}p，实际只有 {got_w}×{got} —— 虚标"
                    ),
                });
            } else {
                findings.push(Finding {
                    level: Level::Ok,
                    text: format!("分辨率 {got_w}×{got}，和名字里的 {want}p 相符"),
                });
            }
        }
        (Some(want), _, _) => findings.push(Finding {
            level: Level::Warn,
            text: format!("名字写 {want}p，但容器里没读到分辨率"),
        }),
        _ => {}
    }

    // 2. 音轨
    if claimed.chinese_audio {
        let has_cn = info.audio_langs.iter().any(|l| is_chinese(l));
        let unknown = info.audio_langs.iter().any(|l| l == "und" || l.is_empty());
        findings.push(if has_cn {
            Finding {
                level: Level::Ok,
                text: format!("有中文音轨（{}）", info.audio_langs.join(" / ")),
            }
        } else if unknown || info.audio_langs.is_empty() {
            // und = 没标语言。这时候不能断言「没有国配」—— 很多压制就是不标。
            Finding {
                level: Level::Warn,
                text: format!(
                    "名字写了国语/中文音轨，但音轨没标语言（{}），验不了",
                    info.audio_langs.join(" / ")
                ),
            }
        } else {
            Finding {
                level: Level::Bad,
                text: format!(
                    "名字写了国语/中文音轨，但音轨只有 {} —— 对不上",
                    info.audio_langs.join(" / ")
                ),
            }
        });
    }

    // 3. 码率。只挑离谱的说 —— 正常范围太宽，硬判会误伤。
    if let (Some(mbps), Some(h)) = (bitrate_mbps, info.height) {
        // 2160p 低于 3 Mbps、1080p 低于 1 Mbps，基本是拉伸上来的垃圾。
        let floor = if h >= 1440 { 3.0 } else { 1.0 };
        if mbps < floor {
            findings.push(Finding {
                level: Level::Bad,
                text: format!("码率只有 {mbps:.1} Mbps，对 {h}p 来说太低了，画质不会好"),
            });
        }
    }

    let verdict = summarize(&findings, info);
    VerifyReport {
        container: info.container.to_string(),
        width: info.width,
        height: info.height,
        duration_secs: info.duration_secs,
        audio_langs: info.audio_langs.clone(),
        bitrate_mbps,
        findings,
        verdict,
    }
}

fn summarize(findings: &[Finding], info: &MediaInfo) -> String {
    let bad = findings.iter().filter(|f| f.level == Level::Bad).count();
    let warn = findings.iter().filter(|f| f.level == Level::Warn).count();

    let spec = match (info.width, info.height, info.duration_secs) {
        (Some(w), Some(h), Some(d)) => format!("{w}×{h}，{:.0} 分钟", d / 60.0),
        (Some(w), Some(h), None) => format!("{w}×{h}"),
        _ => "规格读不全".into(),
    };

    if bad > 0 {
        format!("{spec}。**和名字对不上**，见下面 {bad} 条。")
    } else if warn > 0 {
        format!("{spec}。大体对得上，但有 {warn} 条验不了。")
    } else if findings.is_empty() {
        format!("{spec}。名字里没写规格，没什么可对的。")
    } else {
        format!("{spec}。和名字相符。")
    }
}

/// 容器认不出来时的报告 —— 单独一条路，因为这时候没有任何实测规格。
pub fn unknown_container(name: &str) -> VerifyReport {
    let playable = crate::media::PLAYABLE_HINT
        .iter()
        .any(|e| name.to_ascii_lowercase().ends_with(e));
    VerifyReport {
        container: "认不出".into(),
        width: None,
        height: None,
        duration_secs: None,
        audio_langs: Vec::new(),
        bitrate_mbps: None,
        findings: vec![Finding {
            level: if playable { Level::Warn } else { Level::Bad },
            text: if playable {
                "扩展名是视频，但容器头认不出来 —— 可能是还没下到头部，也可能是我们不认识的格式".into()
            } else {
                "这不是个视频文件".into()
            },
        }],
        verdict: "读不出规格，验不了".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(w: u32, h: u32, mins: f64, langs: &[&str]) -> MediaInfo {
        MediaInfo {
            container: "mkv",
            width: Some(w),
            height: Some(h),
            duration_secs: Some(mins * 60.0),
            audio_langs: langs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn gb(n: f64) -> u64 {
        (n * 1_073_741_824.0) as u64
    }

    #[test]
    fn reads_claimed_height_from_names() {
        assert_eq!(claimed_from_name("Foo.2026.2160p.WEB-DL").height, Some(2160));
        assert_eq!(claimed_from_name("Foo.1080p.BluRay").height, Some(1080));
        assert_eq!(claimed_from_name("Foo.4K.HDR").height, Some(2160));
        assert_eq!(claimed_from_name("Foo.720p").height, Some(720));
        assert_eq!(claimed_from_name("Foo.没写画质").height, None);
    }

    #[test]
    fn reads_claimed_chinese_audio() {
        assert!(claimed_from_name("寒战1994[国语配音+中文字幕]").chinese_audio);
        assert!(claimed_from_name("碟中谍8[国英多音轨]").chinese_audio);
        // 只有中文**字幕**不代表有中文**音轨**，别误报。
        assert!(!claimed_from_name("Foo[简繁英字幕].2160p").chinese_audio);
    }

    /// 这是这个功能最主要的用途。
    #[test]
    fn catches_upscaled_fake() {
        let r = judge(
            &info(1280, 720, 120.0, &["en"]),
            &claimed_from_name("Foo.2026.2160p.WEB-DL"),
            gb(8.0),
        );
        assert!(r.findings.iter().any(|f| f.level == Level::Bad));
        assert!(r.verdict.contains("对不上"), "实际：{}", r.verdict);
    }

    /// 宽银幕会裁掉黑边，2160p 实际是 3840×1608 —— **实测手上 9 个文件里
    /// 有 4 个这样**。按高度硬判会把它们全诬告成虚标。
    #[test]
    fn widescreen_crop_is_not_a_fake() {
        for (w, h, claim) in [
            (3840u32, 1608u32, "Foo.2160p.WEB-DL"),
            (1920, 800, "Foo.1080p.BluRay"),
            (1920, 802, "Foo.1080p.WEB-DL"),
            (1920, 1000, "Foo.1080p.TELESYNC"),
        ] {
            let r = judge(&info(w, h, 120.0, &["en"]), &claimed_from_name(claim), gb(8.0));
            assert!(
                !r.findings.iter().any(|f| f.level == Level::Bad),
                "{w}×{h} 声称 {claim} 被误判成虚标：{:?}",
                r.findings
            );
        }
    }

    /// 真实 MKV 用的是 BCP47 两字母码（zh），不是 ISO-639-2（chi）。
    /// 只认三字母的话「有没有国配」永远判成没有。
    #[test]
    fn recognizes_both_chinese_language_codes() {
        for lang in ["zh", "chi", "zho", "cmn", "zh-CN", "yue"] {
            let r = judge(
                &info(1920, 1080, 120.0, &[lang]),
                &claimed_from_name("Foo[国语配音].1080p"),
                gb(8.0),
            );
            assert!(
                r.findings.iter().any(|f| f.level == Level::Ok && f.text.contains("中文音轨")),
                "{lang} 没被认成中文：{:?}",
                r.findings
            );
        }
    }

    #[test]
    fn catches_missing_chinese_audio() {
        let r = judge(
            &info(1920, 1080, 120.0, &["en", "ja"]),
            &claimed_from_name("Foo[国语配音].1080p"),
            gb(8.0),
        );
        assert!(r.findings.iter().any(|f| f.level == Level::Bad && f.text.contains("对不上")));
    }

    /// und = 没标语言，很多压制就是不标。这时候不能断言「没有国配」。
    #[test]
    fn undetermined_audio_is_not_an_accusation() {
        let r = judge(
            &info(1920, 1080, 120.0, &["und", "und"]),
            &claimed_from_name("Foo[国语配音].1080p"),
            gb(8.0),
        );
        assert!(!r.findings.iter().any(|f| f.level == Level::Bad));
        assert!(r.findings.iter().any(|f| f.level == Level::Warn && f.text.contains("验不了")));
    }

    #[test]
    fn catches_absurd_bitrate() {
        // 2160p 却只有 0.6 GB / 2 小时 ≈ 0.7 Mbps
        let r = judge(
            &info(3840, 2160, 120.0, &["en"]),
            &claimed_from_name("Foo.2160p"),
            gb(0.6),
        );
        assert!(r.findings.iter().any(|f| f.level == Level::Bad && f.text.contains("码率")));
    }

    /// 手上这些真实文件的规格，一个都不该被诬告。
    #[test]
    fn real_files_pass_clean() {
        let cases: &[(&str, u32, u32, f64, &[&str], f64)] = &[
            ("寒战1994.Cold.War.1994.2026.2160p.HQ.WEB-DL.H265.DV.DTS[国语配音+中文字幕]",
             3840, 1608, 117.0, &["und", "und", "und", "zh"], 26.7),
            ("The.Conjuring.Last.Rites.2025.1080p.ATVP.WEB-DL.H.265",
             1920, 802, 135.0, &["en"], 7.5),
            ("[4K][DBD-Raws][名侦探柯南][2160P][BDRip][HEVC-10bit]",
             3840, 2160, 109.0, &["ja", "ja"], 11.4),
        ];
        for (name, w, h, mins, langs, size_gb) in cases {
            let r = judge(&info(*w, *h, *mins, langs), &claimed_from_name(name), gb(*size_gb));
            assert!(
                !r.findings.iter().any(|f| f.level == Level::Bad),
                "真实文件被诬告：{name}\n{:?}",
                r.findings
            );
        }
    }

    /// 名字里什么都没写就没什么可对的，别硬凑结论。
    #[test]
    fn nothing_claimed_means_nothing_to_check() {
        let r = judge(&info(1920, 1080, 100.0, &["en"]), &Claimed::default(), gb(5.0));
        assert!(r.findings.is_empty());
        assert!(r.verdict.contains("没什么可对的"), "实际：{}", r.verdict);
    }
}

// ---------------------------------------------------------------------------
// 取字节
// ---------------------------------------------------------------------------

/// 头部读多少。真实 MKV 的 Tracks 通常在前几百 KB，4 MB 留足余量；
/// 而对一个几十 GB 的种子来说，这点数据几秒就到。
const PROBE_BYTES: u64 = 4 << 20;

/// 验一个任务里最大的那个视频文件。
///
/// **只读头部（必要时加尾部）**，不等整个文件下完 —— 这正是 BT 能按需取
/// piece 的价值所在。librqbit 会把请求到的分片提到最高优先级。
pub async fn verify_torrent(
    engine: &crate::engine::Engine,
    id: crate::engine::TorrentId,
) -> anyhow::Result<VerifyReport> {
    use anyhow::{bail, Context};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let files = engine.files(id)?;
    // 挑选中的、最大的那个 —— 影视种子里那个就是正片，其余是海报和说明。
    let target = files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.selected)
        .max_by_key(|(_, f)| f.len);
    let Some((file_id, file)) = target else {
        bail!("这个任务没有选中任何文件");
    };
    if file.len < 1 << 20 {
        bail!("选中的文件太小（{} 字节），不像视频", file.len);
    }

    let read_window = |from_end: bool| async move {
        let mut stream = engine.open_stream(id, file_id).await?;
        let want = PROBE_BYTES.min(file.len) as usize;
        if from_end {
            stream
                .seek(std::io::SeekFrom::Start(file.len - want as u64))
                .await
                .context("定位到文件尾失败")?;
        }
        let mut buf = vec![0u8; want];
        // read_exact 而不是 read：分片是陆续到的，一次 read 可能只给几 KB。
        stream
            .read_exact(&mut buf)
            .await
            .context("读取失败（分片还没下到？）")?;
        anyhow::Ok(buf)
    };

    let head = read_window(false).await?;
    let probed = match crate::media::probe(&head) {
        crate::media::Probe::NeedTail => {
            // 没做 faststart 的 MP4，moov 在末尾。
            tracing::info!("moov 不在头部，改读文件尾");
            crate::media::probe(&read_window(true).await?)
        }
        other => other,
    };

    let info = match probed {
        crate::media::Probe::Found(i) => i,
        _ => return Ok(unknown_container(&file.name)),
    };

    // 用**任务名**而不是文件名来取「声称的规格」：画质标记通常在任务名上，
    // 内层文件名有时只是个裸片名。
    let torrent_name = engine
        .list()
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| t.name)
        .unwrap_or_default();
    let claimed = claimed_from_name(&format!("{torrent_name} {}", file.name));

    Ok(judge(&info, &claimed, file.len))
}
