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
//! MP4 / MOV / M4V（ISO BMFF 盒子）和 MKV / WebM（EBML）覆盖了影视资源的
//! 绝大多数，两者的分辨率、时长、音轨语言都读得全。另外两种是部分支持：
//!
//! | 容器 | 分辨率 | 时长 | 音轨语言 |
//! |---|---|---|---|
//! | MP4 / MKV | ✓ | ✓ | ✓ |
//! | AVI | ✓ | ✓ | **读不出**（没有标准字段） |
//! | TS / M2TS | **读不出**（在基本流里） | **读不出** | ✓（PMT 里的语言描述符） |
//!
//! 读不出的一律留成 `None` / 空，**不猜**。判读那边会显示「容器里没读到
//! 分辨率」而不是编一个数字出来，理由见 [`parse_ts`] 的文档。

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
    if let Some(info) = parse_avi(bytes) {
        return Probe::Found(info);
    }
    // TS 放最后：它的判据（每 188 字节一个 0x47）比别人的魔数弱，
    // 先让有明确文件头的容器认领。
    if let Some(info) = parse_ts(bytes) {
        return Probe::Found(info);
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

// ---------------------------------------------------------------------------
// AVI（RIFF）
// ---------------------------------------------------------------------------

fn read_u32_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// 遍历一层 RIFF 块：`[4 字节 id][u32 小端 大小][负载]`，负载补齐到偶数字节。
///
/// **那个补齐位不算在 size 里**，忘了跳过的话下一个块的 id 会错开一字节，
/// 整条链就散了。
fn riff_chunks(data: &[u8]) -> Vec<(&[u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let id: &[u8; 4] = data[pos..pos + 4].try_into().unwrap();
        let size = match read_u32_le(data, pos + 4) {
            Some(s) => s as usize,
            None => break,
        };
        let start = pos + 8;
        let end = start.saturating_add(size).min(data.len());
        out.push((id, &data[start..end]));
        if start + size > data.len() {
            break; // 截断了（只取了文件头），到此为止
        }
        pos = start + size + (size & 1);
    }
    out
}

/// LIST 块的负载开头是 4 字节的类型（`hdrl` / `strl` / `movi`），后面才是子块。
fn riff_list<'a>(payload: &'a [u8], want: &[u8; 4]) -> Option<Vec<(&'a [u8; 4], &'a [u8])>> {
    (payload.len() >= 4 && &payload[..4] == want).then(|| riff_chunks(&payload[4..]))
}

/// AVI 的头在 `LIST hdrl` 里：`avih` 给时长，视频流的 `strf`
/// （BITMAPINFOHEADER）给分辨率。
///
/// **分辨率优先取 strf 而不是 avih。** avih 的 dwWidth/dwHeight 是「建议
/// 显示尺寸」，有些封装工具压根不填或填成 0；strf 里的才是这条视频流真实的
/// 像素数。
///
/// **音轨语言读不出来**：AVI 没有标准的每流语言字段（有人用 `strn` 或 INFO
/// 里的 `IAS1`，但那是约定不是规范）。所以这里永远返回空的 `audio_langs` ——
/// 判读那边把「没标语言」当成「验不了」，不会误报成「没有国配」。
fn parse_avi(data: &[u8]) -> Option<MediaInfo> {
    let top = riff_chunks(data);
    let (_, riff) = top.iter().find(|(id, _)| *id == b"RIFF")?;
    if riff.len() < 4 || &riff[..4] != b"AVI " {
        return None;
    }

    let hdrl = riff_chunks(&riff[4..])
        .into_iter()
        .find_map(|(id, p)| (id == b"LIST").then(|| riff_list(p, b"hdrl")).flatten())?;

    let mut info = MediaInfo {
        container: "avi",
        ..Default::default()
    };

    if let Some((_, avih)) = hdrl.iter().find(|(id, _)| *id == b"avih") {
        let us_per_frame = read_u32_le(avih, 0).unwrap_or(0);
        let frames = read_u32_le(avih, 16).unwrap_or(0);
        if us_per_frame > 0 && frames > 0 {
            info.duration_secs = Some(frames as f64 * us_per_frame as f64 / 1e6);
        }
        // 先拿 avih 的尺寸兜底，下面有 strf 就覆盖掉。
        match (read_u32_le(avih, 32), read_u32_le(avih, 36)) {
            (Some(w), Some(h)) if w > 0 && h > 0 => {
                info.width = Some(w);
                info.height = Some(h);
            }
            _ => {}
        }
    }

    for (id, payload) in &hdrl {
        let Some(strl) = (*id == b"LIST").then(|| riff_list(payload, b"strl")).flatten() else {
            continue;
        };
        // strh 的头 4 字节是 fccType：vids / auds / txts
        let is_video = strl
            .iter()
            .find(|(id, _)| *id == b"strh")
            .is_some_and(|(_, h)| h.starts_with(b"vids"));
        if !is_video {
            continue;
        }
        if let Some((_, strf)) = strl.iter().find(|(id, _)| *id == b"strf") {
            // BITMAPINFOHEADER：biWidth 在 +4，biHeight 在 +8，都是有符号的。
            // biHeight 为负表示自上而下存储的位图 —— 取绝对值，不是「负的高度」。
            let w = read_u32_le(strf, 4).map(|v| (v as i32).unsigned_abs());
            let h = read_u32_le(strf, 8).map(|v| (v as i32).unsigned_abs());
            if let (Some(w), Some(h)) = (w, h) {
                if w > 0 && h > 0 {
                    info.width = Some(w);
                    info.height = Some(h);
                }
            }
        }
        break;
    }

    (info.width.is_some() || info.duration_secs.is_some()).then_some(info)
}

// ---------------------------------------------------------------------------
// MPEG-TS
// ---------------------------------------------------------------------------

/// TS 包长。188 是标准；m2ts / 蓝光是每包前面多 4 字节时间戳。
const TS_SIZES: &[(usize, usize)] = &[(188, 0), (192, 4)];

/// 至少连着对上几个包才算数。只看一个 0x47 会把随便什么二进制都认成 TS。
const TS_CONFIRM_PACKETS: usize = 5;

/// 判断是不是 TS，返回 (包长, 包内偏移)。
fn ts_layout(b: &[u8]) -> Option<(usize, usize)> {
    TS_SIZES.iter().copied().find(|&(size, off)| {
        (0..TS_CONFIRM_PACKETS).all(|i| b.get(i * size + off) == Some(&0x47))
    })
}

/// 声明为音频的 stream_type。
///
/// 0x06（私有数据）不在里面：DVB 用它装 AC-3，**也用它装字幕和图文电视**。
/// 只有当它带着 AC-3 描述符时才认，见下面。
const TS_AUDIO_TYPES: &[u8] = &[0x03, 0x04, 0x0F, 0x11, 0x81, 0x87];

/// AC-3 / E-AC-3 描述符标签。带着它的 0x06 流是音频，没有争议。
const DESC_AC3: &[u8] = &[0x6A, 0x7A];
/// ISO_639_language_descriptor。
const DESC_LANG: u8 = 0x0A;

/// 从一段描述符里取 ISO-639 语言码。
fn ts_descriptor_langs(desc: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 2 <= desc.len() {
        let tag = desc[pos];
        let len = desc[pos + 1] as usize;
        let body = match desc.get(pos + 2..pos + 2 + len) {
            Some(b) => b,
            None => break,
        };
        if tag == DESC_LANG {
            // 每 4 字节一组：3 字节语言码 + 1 字节 audio_type
            for g in body.chunks_exact(4) {
                if let Ok(s) = std::str::from_utf8(&g[..3]) {
                    if s.chars().all(|c| c.is_ascii_alphabetic()) {
                        out.push(s.to_ascii_lowercase());
                    }
                }
            }
        }
        pos += 2 + len;
    }
    out
}

fn ts_has_tag(desc: &[u8], tags: &[u8]) -> bool {
    let mut pos = 0usize;
    while pos + 2 <= desc.len() {
        if tags.contains(&desc[pos]) {
            return true;
        }
        pos += 2 + desc[pos + 1] as usize;
    }
    false
}

/// 取出一个包的负载，顺便告诉调用方这个包是不是一段 section 的开头。
fn ts_payload(pkt: &[u8]) -> Option<(&[u8], bool, u16)> {
    if pkt.first() != Some(&0x47) {
        return None;
    }
    let pusi = pkt[1] & 0x40 != 0;
    let pid = (((pkt[1] & 0x1f) as u16) << 8) | pkt[2] as u16;
    let afc = (pkt[3] >> 4) & 0b11;
    let mut at = 4usize;
    if afc & 0b10 != 0 {
        // adaptation_field_length 本身不算在长度里
        at += 1 + *pkt.get(4)? as usize;
    }
    if afc & 0b01 == 0 {
        return None; // 只有 adaptation field，没有负载
    }
    Some((pkt.get(at..)?, pusi, pid))
}

/// 从一个 section 起始包的负载里切出 section 体（去掉 pointer_field 和头）。
///
/// **只处理装得下一个包的 section。** PAT/PMT 一般就几十字节，跨包的极少见；
/// 真跨了就跳过这一份，等下一次重复播出（PSI 每隔几百毫秒就重发一次）。
fn ts_section(payload: &[u8], table_id: u8) -> Option<&[u8]> {
    let ptr = *payload.first()? as usize;
    let sec = payload.get(1 + ptr..)?;
    if *sec.first()? != table_id {
        return None;
    }
    let len = ((((sec.get(1)? & 0x0f) as usize) << 8) | *sec.get(2)? as usize).checked_sub(4)?;
    // 3 字节头 + 内容，末尾 4 字节 CRC 已经在上面减掉了
    sec.get(3..3 + len)
}

/// 解析 TS。
///
/// # 能读出什么
///
/// 音轨语言 —— PMT 里的 `ISO_639_language_descriptor` 是**容器层**的字段，
/// 和 MKV 的 `Language` 一个性质，读出来就是事实。
///
/// # 读不出什么，以及为什么不硬来
///
/// **分辨率和时长不在 TS 容器里。** TS 是个传输流：分辨率藏在视频基本流的
/// SPS（H.264）或序列头（MPEG-2）里，要按位解 Exp-Golomb、还得处理防竞争
/// 字节；时长得靠首尾 PCR 相减，而尾部我们根本没取。那些是解码器的活，
/// 写出来也没有真实样本能验 —— 与其给一个可能编错的数字，不如照这个模块
/// 一贯的规矩：**认不出就说认不出**，判读那边会显示「容器里没读到分辨率」。
fn parse_ts(data: &[u8]) -> Option<MediaInfo> {
    let (size, off) = ts_layout(data)?;

    let mut pmt_pids: Vec<u16> = Vec::new();
    let mut langs: Vec<String> = Vec::new();

    for pkt in data[off..].chunks(size) {
        let Some((payload, pusi, pid)) = ts_payload(pkt) else {
            continue;
        };
        if !pusi {
            continue;
        }

        if pid == 0 {
            // PAT：5 字节头之后是 (program_number, program_map_PID) 对
            if let Some(sec) = ts_section(payload, 0x00) {
                for e in sec.get(5..).unwrap_or_default().chunks_exact(4) {
                    let prog = u16::from_be_bytes([e[0], e[1]]);
                    let map_pid = (((e[2] & 0x1f) as u16) << 8) | e[3] as u16;
                    // program_number 0 是 NIT，不是节目
                    if prog != 0 && !pmt_pids.contains(&map_pid) {
                        pmt_pids.push(map_pid);
                    }
                }
            }
            continue;
        }

        if !pmt_pids.contains(&pid) {
            continue;
        }
        let Some(sec) = ts_section(payload, 0x02) else {
            continue;
        };
        // 5 字节头 + PCR_PID(2) + program_info_length(2)
        let prog_info_len = (((*sec.get(7)? & 0x0f) as usize) << 8) | *sec.get(8)? as usize;
        let mut pos = 9 + prog_info_len;
        while pos + 5 <= sec.len() {
            let stream_type = sec[pos];
            let es_len = (((sec[pos + 3] & 0x0f) as usize) << 8) | sec[pos + 4] as usize;
            let desc = sec.get(pos + 5..pos + 5 + es_len).unwrap_or_default();

            let is_audio = TS_AUDIO_TYPES.contains(&stream_type)
                || (stream_type == 0x06 && ts_has_tag(desc, DESC_AC3));
            if is_audio {
                for l in ts_descriptor_langs(desc) {
                    if !langs.contains(&l) {
                        langs.push(l);
                    }
                }
            }
            pos += 5 + es_len;
        }
    }

    Some(MediaInfo {
        container: if off == 4 { "m2ts" } else { "ts" },
        audio_langs: langs,
        ..Default::default()
    })
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

    // ---- AVI ----

    /// RIFF 块：`[id][u32 小端 大小][负载]`，负载补齐到偶数字节。
    fn riff(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn list(kind: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
        let mut body = kind.to_vec();
        for c in children {
            body.extend_from_slice(c);
        }
        riff(b"LIST", &body)
    }

    /// avih：dwMicroSecPerFrame 在 0，dwTotalFrames 在 16，
    /// dwWidth/dwHeight 在 32/36。全是小端 u32。
    fn avih(us_per_frame: u32, frames: u32, w: u32, h: u32) -> Vec<u8> {
        let mut p = vec![0u8; 56];
        p[0..4].copy_from_slice(&us_per_frame.to_le_bytes());
        p[16..20].copy_from_slice(&frames.to_le_bytes());
        p[32..36].copy_from_slice(&w.to_le_bytes());
        p[36..40].copy_from_slice(&h.to_le_bytes());
        riff(b"avih", &p)
    }

    /// 视频流的 strh + strf。strf 是 BITMAPINFOHEADER：biWidth 在 +4、
    /// biHeight 在 +8，都是 i32。
    fn video_strl(w: i32, h: i32) -> Vec<u8> {
        let mut strh = vec![0u8; 56];
        strh[0..4].copy_from_slice(b"vids");
        let mut strf = vec![0u8; 40];
        strf[0..4].copy_from_slice(&40u32.to_le_bytes());
        strf[4..8].copy_from_slice(&w.to_le_bytes());
        strf[8..12].copy_from_slice(&h.to_le_bytes());
        list(b"strl", &[riff(b"strh", &strh), riff(b"strf", &strf)])
    }

    fn avi_file(children: &[Vec<u8>]) -> Vec<u8> {
        let mut body = b"AVI ".to_vec();
        body.extend_from_slice(&list(b"hdrl", children));
        riff(b"RIFF", &body)
    }

    #[test]
    fn avi_reads_size_and_duration() {
        // 每帧 41708 微秒 ≈ 23.976 fps，10000 帧 ≈ 417 秒
        let file = avi_file(&[avih(41708, 10_000, 1920, 1080), video_strl(1920, 1080)]);

        let Probe::Found(info) = probe(&file) else {
            panic!("没认出 AVI");
        };
        assert_eq!(info.container, "avi");
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)));
        let secs = info.duration_secs.unwrap();
        assert!((secs - 417.08).abs() < 0.01, "时长算错了：{secs}");
        // AVI 没有标准的语言字段，只能是空的 —— 判读那边会当成「验不了」。
        assert!(info.audio_langs.is_empty());
    }

    #[test]
    fn avi_prefers_stream_format_over_main_header() {
        // avih 说 0（有些封装工具就是不填），strf 说 1280×720 —— 以 strf 为准
        let file = avi_file(&[avih(40_000, 100, 0, 0), video_strl(1280, 720)]);
        let Probe::Found(info) = probe(&file) else {
            panic!("没认出 AVI");
        };
        assert_eq!((info.width, info.height), (Some(1280), Some(720)));
    }

    #[test]
    fn avi_negative_height_is_a_top_down_bitmap() {
        // BITMAPINFOHEADER 的 biHeight 为负表示自上而下存储，高度是它的绝对值
        let file = avi_file(&[avih(40_000, 100, 0, 0), video_strl(1920, -1080)]);
        let Probe::Found(info) = probe(&file) else {
            panic!("没认出 AVI");
        };
        assert_eq!(info.height, Some(1080), "负高度该取绝对值，不是当成坏数据");
    }

    // ---- MPEG-TS ----

    /// 一个 188 字节的 TS 包，负载前面带 pointer_field。
    fn ts_packet(pid: u16, section: &[u8]) -> Vec<u8> {
        let mut p = vec![0x47u8];
        // payload_unit_start_indicator + PID 高 5 位
        p.push(0x40 | ((pid >> 8) as u8 & 0x1f));
        p.push(pid as u8);
        p.push(0x10); // 只有负载，无 adaptation field
        p.push(0x00); // pointer_field
        p.extend_from_slice(section);
        p.resize(188, 0xff);
        p
    }

    /// 拼一个 PSI section：table_id + 长度 + 内容 + 4 字节 CRC 占位。
    fn section(table_id: u8, body: &[u8]) -> Vec<u8> {
        let len = body.len() + 4; // 内容 + CRC
        let mut s = vec![table_id, 0xb0 | ((len >> 8) as u8 & 0x0f), len as u8];
        s.extend_from_slice(body);
        s.extend_from_slice(&[0, 0, 0, 0]);
        s
    }

    /// PAT：5 字节头，然后每 4 字节一对 (program_number, program_map_PID)。
    fn pat(program: u16, pmt_pid: u16) -> Vec<u8> {
        let mut body = vec![0u8; 5];
        body.extend_from_slice(&program.to_be_bytes());
        body.push(0xe0 | ((pmt_pid >> 8) as u8 & 0x1f));
        body.push(pmt_pid as u8);
        section(0x00, &body)
    }

    /// PMT：5 字节头 + PCR_PID(2) + program_info_length(2)，然后是流循环。
    /// `streams` 是 (stream_type, PID, 描述符)。
    fn pmt(streams: &[(u8, u16, Vec<u8>)]) -> Vec<u8> {
        let mut body = vec![0u8; 5];
        body.extend_from_slice(&[0xe1, 0x00]); // PCR_PID
        body.extend_from_slice(&[0xf0, 0x00]); // program_info_length = 0
        for (st, pid, desc) in streams {
            body.push(*st);
            body.push(0xe0 | ((pid >> 8) as u8 & 0x1f));
            body.push(*pid as u8);
            body.push(0xf0 | ((desc.len() >> 8) as u8 & 0x0f));
            body.push(desc.len() as u8);
            body.extend_from_slice(desc);
        }
        section(0x02, &body)
    }

    /// ISO_639_language_descriptor：tag 0x0A，每 4 字节一组（3 字节语言码
    /// + 1 字节 audio_type）。
    fn lang_desc(codes: &[&str]) -> Vec<u8> {
        let mut body = Vec::new();
        for c in codes {
            body.extend_from_slice(c.as_bytes());
            body.push(0x00);
        }
        let mut d = vec![0x0A, body.len() as u8];
        d.extend_from_slice(&body);
        d
    }

    fn ts_stream(packets: &[Vec<u8>]) -> Vec<u8> {
        packets.iter().flatten().copied().collect()
    }

    #[test]
    fn ts_reads_audio_languages_from_pmt() {
        let stream = ts_stream(&[
            ts_packet(0, &pat(1, 0x100)),
            ts_packet(
                0x100,
                &pmt(&[
                    (0x1b, 0x101, vec![]),              // H.264 视频
                    (0x0f, 0x102, lang_desc(&["chi"])), // AAC 中文
                    (0x81, 0x103, lang_desc(&["eng"])), // AC-3 英文
                ]),
            ),
            ts_packet(0x101, &[]),
            ts_packet(0x101, &[]),
            ts_packet(0x101, &[]),
        ]);

        let Probe::Found(info) = probe(&stream) else {
            panic!("没认出 TS");
        };
        assert_eq!(info.container, "ts");
        assert_eq!(info.audio_langs, vec!["chi", "eng"]);
        // 分辨率和时长不在容器里，必须留空而不是编一个
        assert_eq!(info.width, None);
        assert_eq!(info.duration_secs, None);
    }

    #[test]
    fn ts_ignores_subtitle_streams_labelled_private_data() {
        // 0x06 私有数据 + 语言描述符，但没有 AC-3 描述符 —— DVB 字幕就长这样。
        // 认成音轨的话，「有没有国配」会被一条中文字幕轨骗过去。
        let stream = ts_stream(&[
            ts_packet(0, &pat(1, 0x100)),
            ts_packet(
                0x100,
                &pmt(&[(0x1b, 0x101, vec![]), (0x06, 0x104, lang_desc(&["chi"]))]),
            ),
            ts_packet(0x101, &[]),
            ts_packet(0x101, &[]),
            ts_packet(0x101, &[]),
        ]);

        let Probe::Found(info) = probe(&stream) else {
            panic!("没认出 TS");
        };
        assert!(
            info.audio_langs.is_empty(),
            "字幕轨不能算成音轨，实际认出了 {:?}",
            info.audio_langs
        );
    }

    #[test]
    fn a_single_stray_0x47_is_not_a_transport_stream() {
        // 只看一个同步字节的话，随便什么二进制都能被认成 TS。
        let mut junk = vec![0u8; 2000];
        junk[0] = 0x47;
        assert_eq!(probe(&junk), Probe::Unknown);
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
