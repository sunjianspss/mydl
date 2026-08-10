//! 跨会话累计每个种子上传了多少。
//!
//! # 为什么要自己攒
//!
//! librqbit 的 `uploaded_bytes` **只统计本次会话**，会话文件里根本没这个
//! 字段。所以「分享率到 2.0 就停做种」的实际含义一直是「本次运行期间上传到
//! 两倍」—— 重启一次就归零，挂了一周的种可能一次都没到过上限。
//!
//! # 增量怎么算才不会错
//!
//! **按种子分别算增量并截断到非负**。会让 `uploaded_bytes` 变小的情况都不该
//! 记成负数或巨大跳变 —— 重启归零、任务被删掉又加回来、重新校验。
//!
//! 这里必须按种子算，不能像 [`crate::stats`] 那样用会话级的总量：分享率阈值
//! 是**逐个种子**判的，只有一个总数没法知道该停哪个。上传量本身没有
//! `progress_bytes` 那种「磁盘上已有多少」的语义，单个种子的 `uploaded_bytes`
//! 在会话内只增不减，所以按种子算是安全的。
//!
//! # 任务消失时不能丢掉基线
//!
//! 种子从列表里消失时，**总量要留着，基线必须清掉**。留着基线的话，同一个
//! 种子被删掉再加回来时 `uploaded_bytes` 从 0 重新开始，而
//! `saturating_sub(旧基线)` 会一直算出 0，一直到重新传够旧基线那么多为止
//! —— 那段时间的上传就凭空丢了。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 最短写盘间隔。调用方每 5 秒来一轮，每轮都写盘纯属糟蹋 SSD；
/// 掉电最多丢一分钟的上传量，对一个分享率阈值来说无所谓。
const SAVE_EVERY: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Entry {
    /// 跨会话累计上传字节。
    total: u64,
    /// 上一轮看到的本会话 `uploaded_bytes`。None = 这个种子当前不在列表里，
    /// 下次出现时重新起基线。
    last: Option<u64>,
}

#[derive(Serialize, Deserialize, Default)]
struct Data {
    /// info-hash（小写）-> 累计量。
    totals: HashMap<String, Entry>,
}

pub struct RatioStore {
    path: PathBuf,
    data: Mutex<Data>,
    last_save: Mutex<Option<Instant>>,
}

impl RatioStore {
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
            last_save: Mutex::new(None),
        }
    }

    /// 记一轮。`current` 是当前每个种子的 (info-hash, 本会话已上传)。
    ///
    /// 不在 `current` 里的种子**保留累计量、清掉基线**，见模块头。
    pub fn record(&self, current: &[(String, u64)]) {
        let mut data = self.data.lock().unwrap();

        for (hash, uploaded) in current {
            let e = data.totals.entry(hash.clone()).or_default();
            // 第一次见到（或刚被加回来）就以当前值起基线，不把历史算成新增。
            let base = e.last.unwrap_or(*uploaded);
            e.total += uploaded.saturating_sub(base);
            e.last = Some(*uploaded);
        }

        let present: std::collections::HashSet<&String> =
            current.iter().map(|(h, _)| h).collect();
        // 走掉的种子清基线；累计量为 0 的顺手删掉，没什么可记的。
        data.totals
            .retain(|h, e| present.contains(h) || e.total > 0);
        for (h, e) in data.totals.iter_mut() {
            if !present.contains(h) {
                e.last = None;
            }
        }

        drop(data);
        self.save_if_due();
    }

    /// 这个种子累计上传了多少。没记录过就是 0。
    pub fn total(&self, hash: &str) -> u64 {
        self.data
            .lock()
            .unwrap()
            .totals
            .get(hash)
            .map(|e| e.total)
            .unwrap_or(0)
    }

    fn save_if_due(&self) {
        let mut last = self.last_save.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < SAVE_EVERY) {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        self.save();
    }

    /// 写盘失败不影响做种，最多是累计量退回上次存盘的值。
    pub fn save(&self) {
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
            Err(e) => tracing::warn!("序列化累计上传量失败：{e:#}"),
        }
    }
}

pub fn ratio_path(config_dir: &Path) -> PathBuf {
    config_dir.join("upload_totals.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mydl-ratio-{}-{tag}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn accumulates_deltas_not_absolute_values() {
        let s = RatioStore::load(tmp("delta"));

        // 第一轮只起基线，不把历史上传算成新增
        s.record(&[("a".into(), 500)]);
        assert_eq!(s.total("a"), 0);

        s.record(&[("a".into(), 800)]);
        assert_eq!(s.total("a"), 300);

        s.record(&[("a".into(), 1000)]);
        assert_eq!(s.total("a"), 500);
    }

    #[test]
    fn restart_does_not_lose_or_double_count() {
        let path = tmp("restart");

        let s = RatioStore::load(path.clone());
        s.record(&[("a".into(), 0)]);
        s.record(&[("a".into(), 1000)]);
        s.save();
        assert_eq!(s.total("a"), 1000);

        // 重启：uploaded_bytes 归零，累计量必须留着，而且不能因为
        // 「现在比上次小」就乱记。
        let s = RatioStore::load(path.clone());
        assert_eq!(s.total("a"), 1000);
        s.record(&[("a".into(), 0)]);
        assert_eq!(s.total("a"), 1000, "重启那一轮不该有新增");
        s.record(&[("a".into(), 400)]);
        assert_eq!(s.total("a"), 1400);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn removed_then_readded_keeps_total_and_resets_baseline() {
        let s = RatioStore::load(tmp("readd"));
        s.record(&[("a".into(), 0)]);
        s.record(&[("a".into(), 5000)]);
        assert_eq!(s.total("a"), 5000);

        // 从列表里消失（用户删了任务）
        s.record(&[]);
        assert_eq!(s.total("a"), 5000, "累计量不该跟着任务一起没了");

        // 又加回来：本会话上传量从 0 重新开始。留着旧基线的话，这里
        // 要重新传够 5000 才会再有增量 —— 那段上传就凭空丢了。
        s.record(&[("a".into(), 0)]);
        s.record(&[("a".into(), 700)]);
        assert_eq!(s.total("a"), 5700);
    }

    #[test]
    fn forgets_torrents_that_never_uploaded() {
        let s = RatioStore::load(tmp("forget"));
        s.record(&[("a".into(), 0), ("b".into(), 0)]);
        s.record(&[("a".into(), 100)]);
        // a 有累计量，留着；b 一个字节没传过，没必要一直占地方
        assert_eq!(s.total("a"), 100);
        assert!(!s.data.lock().unwrap().totals.contains_key("b"));
    }
}
