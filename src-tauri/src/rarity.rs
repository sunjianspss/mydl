//! 稀缺度：你手上这份，全网还有几份？
//!
//! # 为什么值得做
//!
//! 做种现在是个没有反馈的义务：界面只显示「做种中 · 0 peers」，你不知道
//! 自己在给谁、给多少、有没有意义。而客户端其实**知道哪些是稀有的** ——
//! `health.rs` 每半小时 scrape 一次，做种人数是现成的。
//!
//! 一部有 800 个做种者的热门片，你退出不退出没人在乎；而一部全网只剩 3 份
//! 的老片，你就是那 1/3。这两件事在界面上现在长得一模一样。
//!
//! # 为什么机制是「暂停」而不是「分配带宽」
//!
//! 最想做的是把上传带宽向稀有的那些倾斜。做不到：librqbit v9 的
//! per-torrent `ratelimits` 在 `ManagedTorrentOptions` 里，而整个结构是
//! `pub(crate)`，外部够不着（确认过）。只有全局那一个 `Session.ratelimits`。
//!
//! 所以能用的只有一个粗粒度开关：**暂停不稀缺的，把全局那份预算让给稀缺的**。
//! 粗，但方向是对的，而且是现有 API 就能做到的唯一一种分配。

use serde::Serialize;

use crate::health::Sample;

/// 做种数少于这个就算稀缺 —— 包括你自己在内。
///
/// 5 是拍的，但有依据：低于这个数，任何一个人退出都会让可用性明显变差；
/// 高于这个数，你在不在基本没区别。界面上会把实际数字显示出来，
/// 用户可以自己判断这条线画得对不对。
const RARE_AT_OR_BELOW: u32 = 5;

/// 至少要几个可信样本才敢下结论。一次 scrape 抖动不能定性。
const MIN_SAMPLES: usize = 3;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Rarity {
    /// 样本不够，不下结论。
    Unknown,
    /// 全网寥寥几份，你这份有分量。
    Rare,
    /// 做种者充足，你在不在区别不大。
    Common,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub rarity: Rarity,
    /// 用来判断的做种数（窗口内中位数）。中位数而不是最新值 ——
    /// 做种数抖得厉害，实测变异系数 34%~151%。
    pub seeders: Option<u32>,
    /// 一句话。
    pub summary: String,
}

/// 判稀缺度。纯函数。
///
/// `seeders` 用**中位数**：单次 scrape 的抖动太大，拿最新值判会让同一个
/// 种子在「稀有」和「充足」之间反复横跳，而这个判断要拿来决定暂停谁。
pub fn judge(history: &[Sample]) -> Verdict {
    let mut valid: Vec<u32> = history
        .iter()
        .filter(|s| s.trackers_ok > 0)
        .map(|s| s.seeders)
        .collect();

    if valid.len() < MIN_SAMPLES {
        return Verdict {
            rarity: Rarity::Unknown,
            seeders: None,
            summary: "还没采够数据，判不出稀缺度".into(),
        };
    }

    valid.sort_unstable();
    let median = valid[valid.len() / 2];

    if median <= RARE_AT_OR_BELOW {
        // 做种数**包含你自己**，所以「除你之外」要减一。
        let others = median.saturating_sub(1);
        let summary = if others == 0 {
            "全网只有你这一份在做种 —— 你退出它就没了".to_string()
        } else {
            format!("全网只有 {median} 份在做种，你是其中之一（另外 {others} 份）")
        };
        Verdict {
            rarity: Rarity::Rare,
            seeders: Some(median),
            summary,
        }
    } else {
        Verdict {
            rarity: Rarity::Common,
            seeders: Some(median),
            summary: format!("{median} 个人在做种，不缺你这一份"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(seeders: u32) -> Sample {
        Sample {
            ts: 0,
            seeders,
            leechers: 1,
            trackers_ok: 2,
            ..Default::default()
        }
    }

    #[test]
    fn not_enough_samples_stays_unknown() {
        assert_eq!(judge(&[s(1), s(1)]).rarity, Rarity::Unknown);
    }

    /// 全网只剩你一份 —— 这句话要说得足够重，它是这个功能的意义所在。
    #[test]
    fn last_copy_gets_a_strong_message() {
        let v = judge(&[s(1), s(1), s(1), s(1)]);
        assert_eq!(v.rarity, Rarity::Rare);
        assert!(v.summary.contains("只有你这一份"), "实际：{}", v.summary);
    }

    /// 做种数**包含自己**，所以「另外几份」要减一。写错的话会把
    /// 「全网 3 份」说成「另外还有 3 份」，凭空多算一份。
    #[test]
    fn counts_others_excluding_yourself() {
        let v = judge(&[s(3), s(3), s(3)]);
        assert!(v.summary.contains("另外 2 份"), "实际：{}", v.summary);
    }

    #[test]
    fn plenty_of_seeders_is_common() {
        let v = judge(&[s(40), s(38), s(45)]);
        assert_eq!(v.rarity, Rarity::Common);
        assert_eq!(v.seeders, Some(40));
    }

    /// 用中位数而不是最新值：做种数抖得厉害（实测变异系数 34%~151%），
    /// 拿最新值判会让同一个种子在两个结论之间反复横跳，而这个判断要拿来
    /// 决定暂停谁。
    #[test]
    fn uses_median_not_the_latest_sample() {
        // 长期 2 个做种，最后一次 scrape 抖到 30
        let mut h: Vec<Sample> = (0..9).map(|_| s(2)).collect();
        h.push(s(30));
        let v = judge(&h);
        assert_eq!(v.rarity, Rarity::Rare, "一次抖动不该翻转结论");
        assert_eq!(v.seeders, Some(2));
    }

    /// tracker 全没应答的样本不算数 —— 那会得到 0 做种，
    /// 把每个种子都判成「全网仅存」。
    #[test]
    fn untrustworthy_samples_are_ignored() {
        let bad = Sample {
            ts: 0,
            seeders: 0,
            leechers: 0,
            trackers_ok: 0,
            ..Default::default()
        };
        let v = judge(&[s(40), s(38), s(45), bad, bad, bad]);
        assert_eq!(v.rarity, Rarity::Common, "断网的样本不该把它判成稀有");
    }

    #[test]
    fn boundary_is_inclusive() {
        assert_eq!(judge(&[s(5), s(5), s(5)]).rarity, Rarity::Rare);
        assert_eq!(judge(&[s(6), s(6), s(6)]).rarity, Rarity::Common);
    }
}
