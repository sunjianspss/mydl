//! 从视频文件的**容器头部**读出真实的分辨率、时长和音轨语言。
//!
//! # 为什么不需要解码器
//!
//! 分辨率和时长是容器的**元数据**，不在压缩数据里：MP4 放在 `moov` 里的
//! `tkhd`/`mvhd`，MKV 放在 EBML 的 `Tracks`/`Info`。读几 MB 头部就能拿到，
//! 一帧都不用解。
//!
//! 这件事对 BT 特别合适：**可以按需取任意 piece**。想验一个 14 GB 的种子
//! 是不是虚标，取头部那几 MB 就够，不用等它下完。
//!
//! 打包 ffmpeg 的话体积要翻好几倍，还得处理三个平台的动态库和授权 ——
//! 而我们只需要几个整数。
//!
//! # 覆盖范围
//!
//! MP4 / MOV / M4V（ISO BMFF 盒子）和 MKV / WebM（EBML）。这两类覆盖了
//! 影视资源的绝大多数。AVI / TS 之类没做 —— 认不出来时老实说「认不出」，
//! 不猜。

/// 常见的视频扩展名。只用来判断「这本来该是个视频吗」，
/// 真正的判断永远以容器头为准。
pub const PLAYABLE_HINT: &[&str] = &[
    ".mkv", ".mp4", ".m4v", ".mov", ".webm", ".avi", ".ts", ".m2ts", ".wmv", ".flv",
];

/// 从容器头里读出来的事实。全是 Option —— 缺字段是常态，不能因为少一个
/// 就整个失败。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaInfo {
    pub container: &'static str,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_secs: Option<f64>,
    /// 音轨语言（ISO-639，小写）。顺序按容器里的顺序。
    pub audio_langs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    Found(MediaInfo),
    /// 是 MP4，但 `moov` 不在头部 —— 没做 faststart 的话它在文件末尾。
    /// 调用方该去读文件尾再解一次。
    NeedTail,
    /// 认不出的容器。**不猜**。
    Unknown,
}

/// 解析一段字节。可以是文件头，也可以是文件尾（给没做 faststart 的 MP4）。
pub fn probe(bytes: &[u8]) -> Probe {
    if is_matroska(bytes) {
        return match parse_matroska(bytes) {
            Some(info) => Probe::Found(info),
            None => Probe::Unknown,
        };
    }
    if let Some(r) = probe_mp4(bytes) {
        return r;
    }
    Probe::Unknown
}

// ---------------------------------------------------------------------------
// MP4 / ISO BMFF
// ---------------------------------------------------------------------------

/// 盒子结构：`[u32 大小][4 字节类型][负载]`。大小含头本身；
/// 为 1 时后面跟一个 u64 的扩展大小；为 0 表示「一直到文件尾」。
fn boxes(data: &[u8]) -> Vec<(&[u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let kind: &[u8; 4] = data[pos + 4..pos + 8].try_into().unwrap();

        let (header, size) = match size {
            // 0 = 到文件尾
            0 => (8, data.len() - pos),
            1 => {
                if pos + 16 > data.len() {
                    break;
                }
                let big = u64::from_be_bytes(data[pos + 8..pos + 16].try_into().unwrap()) as usize;
                (16, big)
            }
            s if s < 8 => break, // 非法，别死循环
            s => (8, s),
        };

        let end = pos.saturating_add(size).min(data.len());
        if end <= pos + header {
            break;
        }
        out.push((kind, &data[pos + header..end]));
        // 截断的盒子（头部只读了一半）就到此为止。
        if pos + size > data.len() {
            break;
        }
        pos += size;
    }
    out
}

fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(k, _)| *k == kind).map(|(_, v)| v)
}

fn probe_mp4(data: &[u8]) -> Option<Probe> {
    let top = boxes(data);
    let looks_mp4 = top.iter().any(|(k, _)| *k == b"ftyp" || *k == b"moov");
    if !looks_mp4 {
        return None;
    }

    let Some(moov) = top.iter().find(|(k, _)| *k == b"moov").map(|(_, v)| v) else {
        // 有 ftyp 没 moov —— 没做 faststart，moov 在文件末尾。
        return Some(Probe::NeedTail);
    };

    let mut info = MediaInfo {
        container: "mp4",
        ..Default::default()
    };

    // mvhd：版本 0 是 u32 时间戳，版本 1 是 u64。timescale 和 duration 的
    // 偏移随版本变，读错了时长会差好几个数量级。
    if let Some(mvhd) = find(moov, b"mvhd") {
        if let Some(v) = mvhd.first() {
            let (ts_at, dur_at) = if *v == 1 { (20, 24) } else { (12, 16) };
            let scale = read_u32(mvhd, ts_at);
            let dur = if *v == 1 {
                read_u64(mvhd, dur_at)
            } else {
                read_u32(mvhd, dur_at).map(u64::from)
            };
            if let (Some(scale), Some(dur)) = (scale, dur) {
                if scale > 0 {
                    info.duration_secs = Some(dur as f64 / scale as f64);
                }
            }
        }
    }

    for (kind, trak) in boxes(moov) {
        if kind != b"trak" {
            continue;
        }
        let handler = find(trak, b"mdia")
            .and_then(|m| find(m, b"hdlr"))
            // hdlr：4 字节版本+flags，4 字节预定义，然后才是 handler type
            .and_then(|h| h.get(8..12));

        match handler {
            Some(b"vide") => {
                // tkhd 末尾是 16.16 定点的宽高。取高 16 位就是像素值。
                if let Some(tkhd) = find(trak, b"tkhd") {
                    let n = tkhd.len();
                    if n >= 8 {
                        let w = read_u32(tkhd, n - 8).map(|v| v >> 16);
                        let h = read_u32(tkhd, n - 4).map(|v| v >> 16);
                        // 有些轨道的 tkhd 宽高是 0（比如纯数据轨），别覆盖真值。
                        if let (Some(w), Some(h)) = (w, h) {
                            if w > 0 && h > 0 {
                                info.width = Some(w);
                                info.height = Some(h);
                            }
                        }
                    }
                }
            }
            Some(b"soun") => {
                if let Some(lang) = find(trak, b"mdia")
                    .and_then(|m| find(m, b"mdhd"))
                    .and_then(mdhd_language)
                {
                    info.audio_langs.push(lang);
                }
            }
            _ => {}
        }
    }

    Some(Probe::Found(info))
}

/// mdhd 里的语言是**打包成 3 个 5 bit** 的 ISO-639-2，每个值加了 0x60。
fn mdhd_language(mdhd: &[u8]) -> Option<String> {
    let version = *mdhd.first()?;
    let at = if version == 1 { 28 } else { 20 };
    let packed = u16::from_be_bytes(mdhd.get(at..at + 2)?.try_into().ok()?);
    let chars: Vec<char> = (0..3)
        .rev()
        .map(|i| (((packed >> (i * 5)) & 0x1f) as u8 + 0x60) as char)
        .collect();
    let s: String = chars.into_iter().collect();
    s.chars().all(|c| c.is_ascii_lowercase()).then_some(s)
}

fn read_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

// ---------------------------------------------------------------------------
// Matroska / WebM（EBML）
// ---------------------------------------------------------------------------

fn is_matroska(b: &[u8]) -> bool {
    b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
}

/// 读一个 EBML 变长整数。返回 (值, 占了几字节)。
///
/// `keep_marker` 区分两种用法：**元素 ID 要保留前导标记位**（那是 ID 的一
/// 部分），**长度值要去掉**。这两者混淆的话所有 ID 都会对不上。
fn vint(b: &[u8], at: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let first = *b.get(at)?;
    if first == 0 {
        return None; // 非法，别当成超长元素读下去
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || at + len > b.len() {
        return None;
    }
    let mut v = if keep_marker {
        first as u64
    } else {
        (first as u64) & ((1u64 << (8 - len)) - 1)
    };
    for i in 1..len {
        v = (v << 8) | b[at + i] as u64;
    }
    Some((v, len))
}

/// 遍历一层元素，对每个 (id, 负载) 调 `f`。
fn ebml_children(b: &[u8], mut f: impl FnMut(u64, &[u8])) {
    let mut pos = 0usize;
    while pos < b.len() {
        let Some((id, id_len)) = vint(b, pos, true) else {
            return;
        };
        let Some((size, size_len)) = vint(b, pos + id_len, false) else {
            return;
        };
        let start = pos + id_len + size_len;
        // 未知长度（全 1）在 Segment 上很常见 —— 当成「一直到结尾」。
        let unknown = size == (1u64 << (7 * size_len as u32)) - 1;
        let end = if unknown {
            b.len()
        } else {
            start.saturating_add(size as usize).min(b.len())
        };
        if start > b.len() || end < start {
            return;
        }
        f(id, &b[start..end]);
        if unknown {
            return;
        }
        pos = end;
    }
}

fn ebml_uint(b: &[u8]) -> Option<u64> {
    (!b.is_empty() && b.len() <= 8).then(|| b.iter().fold(0u64, |a, x| (a << 8) | *x as u64))
}

fn ebml_float(b: &[u8]) -> Option<f64> {
    match b.len() {
        4 => Some(f32::from_be_bytes(b.try_into().ok()?) as f64),
        8 => Some(f64::from_be_bytes(b.try_into().ok()?)),
        _ => None,
    }
}

fn parse_matroska(data: &[u8]) -> Option<MediaInfo> {
    let mut info = MediaInfo {
        container: "mkv",
        ..Default::default()
    };
    // 默认 1ms，和 Matroska 规范一致。
    let mut timecode_scale = 1_000_000f64;
    let mut duration_ticks: Option<f64> = None;

    ebml_children(data, |id, body| {
        if id != 0x1853_8067 {
            return; // Segment
        }
        ebml_children(body, |id, body| match id {
            0x1549_A966 => ebml_children(body, |id, body| match id {
                0x2AD7_B1 => {
                    if let Some(v) = ebml_uint(body) {
                        timecode_scale = v as f64;
                    }
                }
                0x4489 => duration_ticks = ebml_float(body),
                _ => {}
            }),
            0x1654_AE6B => ebml_children(body, |id, body| {
                if id != 0xAE {
                    return; // TrackEntry
                }
                let mut track_type = 0u64;
                let mut lang: Option<String> = None;
                let mut wh: Option<(u32, u32)> = None;

                ebml_children(body, |id, body| match id {
                    0x83 => track_type = ebml_uint(body).unwrap_or(0),
                    // Language 缺省是 eng；LanguageBCP47（0x22B59D）优先。
                    0x22B5_9C | 0x22B5_9D => {
                        if let Ok(s) = std::str::from_utf8(body) {
                            let s = s.trim_end_matches('\0').to_ascii_lowercase();
                            if !s.is_empty() {
                                lang = Some(s);
                            }
                        }
                    }
                    0xE0 => {
                        let (mut w, mut h) = (0u32, 0u32);
                        ebml_children(body, |id, body| match id {
                            0xB0 => w = ebml_uint(body).unwrap_or(0) as u32,
                            0xBA => h = ebml_uint(body).unwrap_or(0) as u32,
                            _ => {}
                        });
                        if w > 0 && h > 0 {
                            wh = Some((w, h));
                        }
                    }
                    _ => {}
                });

                match track_type {
                    1 => {
                        if let Some((w, h)) = wh {
                            // 多视频轨时以第一条为准。
                            if info.width.is_none() {
                                info.width = Some(w);
                                info.height = Some(h);
                            }
                        }
                    }
                    2 => info.audio_langs.push(lang.unwrap_or_else(|| "eng".into())),
                    _ => {}
                }
            }),
            _ => {}
        });
    });

    if let Some(ticks) = duration_ticks {
        info.duration_secs = Some(ticks * timecode_scale / 1e9);
    }
    // 一个字段都没读出来就别谎称认识它。
    (info.width.is_some() || info.duration_secs.is_some()).then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- MP4 ----

    fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut p = vec![0u8; 100];
        p[0] = 0; // version
        p[12..16].copy_from_slice(&timescale.to_be_bytes());
        p[16..20].copy_from_slice(&duration.to_be_bytes());
        mp4_box(b"mvhd", &p)
    }

    fn video_trak(w: u32, h: u32) -> Vec<u8> {
        // tkhd 末尾 8 字节是 16.16 定点的宽高
        let mut tkhd = vec![0u8; 84];
        let n = tkhd.len();
        tkhd[n - 8..n - 4].copy_from_slice(&(w << 16).to_be_bytes());
        tkhd[n - 4..].copy_from_slice(&(h << 16).to_be_bytes());

        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(b"vide");
        let mdia = mp4_box(b"mdia", &mp4_box(b"hdlr", &hdlr));

        let mut trak = mp4_box(b"tkhd", &tkhd);
        trak.extend_from_slice(&mdia);
        mp4_box(b"trak", &trak)
    }

    fn audio_trak(lang: &str) -> Vec<u8> {
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(b"soun");

        // mdhd v0：语言在偏移 20，3 个 5bit，每个减 0x60
        let mut mdhd = vec![0u8; 24];
        let b = lang.as_bytes();
        let packed = (((b[0] - 0x60) as u16) << 10)
            | (((b[1] - 0x60) as u16) << 5)
            | ((b[2] - 0x60) as u16);
        mdhd[20..22].copy_from_slice(&packed.to_be_bytes());

        let mut mdia = mp4_box(b"hdlr", &hdlr);
        mdia.extend_from_slice(&mp4_box(b"mdhd", &mdhd));
        mp4_box(b"trak", &mp4_box(b"mdia", &mdia))
    }

    #[test]
    fn reads_mp4_resolution_duration_and_audio() {
        let mut moov = mvhd_v0(1000, 7_200_000); // 7200 秒
        moov.extend_from_slice(&video_trak(3840, 2160));
        moov.extend_from_slice(&audio_trak("chi"));
        moov.extend_from_slice(&audio_trak("eng"));

        let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0");
        file.extend_from_slice(&mp4_box(b"moov", &moov));

        let Probe::Found(info) = probe(&file) else {
            panic!("没认出 MP4");
        };
        assert_eq!(info.container, "mp4");
        assert_eq!((info.width, info.height), (Some(3840), Some(2160)));
        assert_eq!(info.duration_secs, Some(7200.0));
        assert_eq!(info.audio_langs, vec!["chi", "eng"]);
    }

    /// 没做 faststart 的 MP4：`moov` 在文件末尾，头部只有 ftyp + mdat。
    /// 必须说「要读文件尾」而不是「认不出」—— 后者会让调用方放弃。
    #[test]
    fn mp4_without_faststart_asks_for_tail() {
        let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0");
        file.extend_from_slice(&mp4_box(b"mdat", &[0u8; 64]));
        assert_eq!(probe(&file), Probe::NeedTail);
    }

    /// mvhd 版本 1 的 timescale/duration 偏移不一样。读错的话时长会差几个
    /// 数量级，而时长正是用来判「是不是完整片子」的。
    #[test]
    fn mp4_mvhd_version1_offsets() {
        let mut p = vec![0u8; 120];
        p[0] = 1;
        p[20..24].copy_from_slice(&1000u32.to_be_bytes());
        p[24..32].copy_from_slice(&5_400_000u64.to_be_bytes());
        let mut moov = mp4_box(b"mvhd", &p);
        moov.extend_from_slice(&video_trak(1920, 1080));

        let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0");
        file.extend_from_slice(&mp4_box(b"moov", &moov));

        let Probe::Found(info) = probe(&file) else {
            panic!("没认出");
        };
        assert_eq!(info.duration_secs, Some(5400.0));
    }

    // ---- MKV ----

    fn ebml_id(id: u64) -> Vec<u8> {
        let mut v = Vec::new();
        let bytes = id.to_be_bytes();
        let start = bytes.iter().position(|b| *b != 0).unwrap_or(7);
        v.extend_from_slice(&bytes[start..]);
        v
    }

    fn elem(id: u64, payload: &[u8]) -> Vec<u8> {
        let mut v = ebml_id(id);
        // 长度用 1 字节形式（0x80 | len），够测试用
        assert!(payload.len() < 0x7f, "测试数据太大");
        v.push(0x80 | payload.len() as u8);
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn reads_mkv_resolution_duration_and_audio() {
        let info_el = elem(
            0x1549_A966,
            &[
                elem(0x2AD7_B1, &1_000_000u32.to_be_bytes()[1..]).as_slice(), // 1ms
                elem(0x4489, &(7_200_000f64).to_be_bytes()).as_slice(),
            ]
            .concat(),
        );

        let video = elem(
            0xAE,
            &[
                elem(0x83, &[1]).as_slice(),
                elem(
                    0xE0,
                    &[
                        elem(0xB0, &3840u16.to_be_bytes()).as_slice(),
                        elem(0xBA, &2160u16.to_be_bytes()).as_slice(),
                    ]
                    .concat(),
                )
                .as_slice(),
            ]
            .concat(),
        );
        let audio = elem(
            0xAE,
            &[
                elem(0x83, &[2]).as_slice(),
                elem(0x22B5_9C, b"chi").as_slice(),
            ]
            .concat(),
        );
        let tracks = elem(0x1654_AE6B, &[video.as_slice(), audio.as_slice()].concat());
        let segment = elem(0x1853_8067, &[info_el.as_slice(), tracks.as_slice()].concat());

        let mut file = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 0, 0, 0, 0];
        file.extend_from_slice(&segment);

        let Probe::Found(info) = probe(&file) else {
            panic!("没认出 MKV");
        };
        assert_eq!(info.container, "mkv");
        assert_eq!((info.width, info.height), (Some(3840), Some(2160)));
        assert_eq!(info.duration_secs, Some(7200.0));
        assert_eq!(info.audio_langs, vec!["chi"]);
    }

    /// 音轨没写 Language 时 Matroska 的缺省是 eng。漏掉这条会让「声称国配
    /// 但没有中文轨」的判断出现假阴性。
    #[test]
    fn mkv_audio_without_language_defaults_to_eng() {
        let audio = elem(0xAE, &elem(0x83, &[2]));
        let video = elem(
            0xAE,
            &[
                elem(0x83, &[1]).as_slice(),
                elem(
                    0xE0,
                    &[
                        elem(0xB0, &1280u16.to_be_bytes()).as_slice(),
                        elem(0xBA, &720u16.to_be_bytes()).as_slice(),
                    ]
                    .concat(),
                )
                .as_slice(),
            ]
            .concat(),
        );
        let tracks = elem(0x1654_AE6B, &[video.as_slice(), audio.as_slice()].concat());
        let segment = elem(0x1853_8067, &tracks);
        let mut file = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 0, 0, 0, 0];
        file.extend_from_slice(&segment);

        let Probe::Found(info) = probe(&file) else {
            panic!("没认出");
        };
        assert_eq!(info.audio_langs, vec!["eng"]);
    }

    // ---- 兜底 ----

    /// 认不出来必须说「认不出」，不能瞎猜 —— 猜错了会诬告一个好种子。
    #[test]
    fn unknown_container_is_admitted() {
        assert_eq!(probe(b"MZ\x90\x00this is a windows exe"), Probe::Unknown);
        assert_eq!(probe(b""), Probe::Unknown);
        assert_eq!(probe(&[0u8; 512]), Probe::Unknown);
    }

    /// 截断的输入（只下到一半）不能 panic，也不能死循环。
    #[test]
    fn truncated_input_does_not_hang() {
        let mut moov = mvhd_v0(1000, 1000);
        moov.extend_from_slice(&video_trak(1920, 1080));
        let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0");
        file.extend_from_slice(&mp4_box(b"moov", &moov));

        for cut in [9, 16, 32, 64, file.len() / 2, file.len() - 1] {
            let _ = probe(&file[..cut.min(file.len())]);
        }
    }

    /// 声明了超大长度的畸形盒子不能让我们读越界。
    #[test]
    fn bogus_box_size_is_survivable() {
        let mut file = mp4_box(b"ftyp", b"isom\0\0\0\0");
        file.extend_from_slice(&u32::MAX.to_be_bytes());
        file.extend_from_slice(b"moov");
        file.extend_from_slice(&[0u8; 16]);
        let _ = probe(&file);
    }
}
