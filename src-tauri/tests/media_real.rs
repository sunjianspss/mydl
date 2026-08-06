//! 拿磁盘上真实的影视文件验容器解析。
//!
//! 单测用的是自己造的 fixture —— 造得出来只说明我理解的格式自洽，不说明
//! 真实压制长这样。真文件的头部有各种花样：多音轨、附件、SeekHead、
//! 巨大的 CueS、没做 faststart 的 MP4……
//!
//! ```
//! cargo test --test media_real -- --ignored --nocapture
//! ```
use mydl_lib::media::{probe, Probe};
use std::io::{Read, Seek, SeekFrom};

/// 头部读多少。真实 MKV 的 Tracks 通常在前几百 KB，留足余量。
const HEAD: usize = 4 << 20;

fn read_at(path: &std::path::Path, from_end: bool) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let n = HEAD.min(len as usize);
    if from_end {
        f.seek(SeekFrom::End(-(n as i64)))?;
    }
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

#[test]
#[ignore = "需要本地有下好的影视文件"]
fn parses_real_files() {
    let dir = std::env::var("MYDL_MEDIA_DIR")
        .unwrap_or_else(|_| format!("{}/Downloads", std::env::var("HOME").unwrap()));

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for entry in walkdir(std::path::Path::new(&dir), 2) {
        let ext = entry
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "mkv" | "mp4" | "m4v" | "mov") {
            files.push(entry);
        }
    }
    if files.is_empty() {
        eprintln!("{dir} 下没有视频文件，跳过");
        return;
    }

    let mut parsed = 0;
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy();
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        let Ok(head) = read_at(f, false) else { continue };

        let mut result = probe(&head);
        // 没做 faststart 的 MP4：moov 在末尾，再读一次文件尾。
        if result == Probe::NeedTail {
            eprintln!("  （moov 不在头部，改读文件尾）");
            if let Ok(tail) = read_at(f, true) {
                result = probe(&tail);
            }
        }

        match result {
            Probe::Found(i) => {
                parsed += 1;
                let mins = i.duration_secs.unwrap_or(0.0) / 60.0;
                let mbps = if mins > 0.0 {
                    size as f64 * 8.0 / (mins * 60.0) / 1e6
                } else {
                    0.0
                };
                eprintln!(
                    "  [{}] {}x{}  {:.0} 分钟  {:.1} Mbps  音轨 {:?}",
                    i.container,
                    i.width.unwrap_or(0),
                    i.height.unwrap_or(0),
                    mins,
                    mbps,
                    i.audio_langs
                );
            }
            Probe::NeedTail => eprintln!("  文件尾也没找到 moov"),
            Probe::Unknown => eprintln!("  认不出容器"),
        }
        // 中文文件名不能按字节切，会切在字符中间 panic。
        eprintln!("      {}", name.chars().take(64).collect::<String>());
    }

    eprintln!("\n{parsed}/{} 个文件解析成功", files.len());
    assert!(parsed > 0, "一个真实文件都没解析出来，解析器多半是错的");
}

fn walkdir(dir: &std::path::Path, depth: usize) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth > 0 {
            out.extend(walkdir(&p, depth - 1));
        } else if p.is_file() {
            out.push(p);
        }
    }
    out
}
