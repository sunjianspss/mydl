//! 从采样历史里算出「还要多久」和「卡了多久」。
//!
//! # 为什么这里没有「完成概率」
//!
//! 一开始想做的是「72 小时内下完的概率 12%」这种东西。攒了两天数据之后
//! 实测了一下，**做种数序列的噪声压倒一切**：
//!
//! ```text
//! 种子          样本  中位做种  标准差  变异系数   线性外推「归零」
//! 碟中谍8         90       4     1.8     46%      3.4 天
//! 寒战1994        90       3     1.7     55%      上升/持平
//! 星球大战        90       7     2.3     34%     33.9 天
//! ```
//!
//! 碟中谍8 那个「3.4 天后归零」是噪声不是趋势 —— 变异系数 46%，而它已经在
//! 2~6 个做种之间活了好几天；换一个 40 小时窗口能算出完全不同的天数。
//!
//! **用这种数据报一个百分比是在编造精度**，而这个数字会让人决定删掉几十 GB。
//! 所以这里一个概率都不出。
//!
//! # 那报什么
//!
//! 报**测量值**，不报预测：
//!
//! - **实测吞吐量** —— 两次采样之间进度涨了多少。这是实打实测出来的，
//!   而且直接回答「还要多久」，比 swarm 规模靠谱得多。
//! - **卡了多久** —— 进度上次变化是什么时候。这个信息现在完全不可见。
//! - **做种数的区间和中位数** —— 而不是拟合一条趋势线。噪声这么大的时候，
//!   「过去 40 小时 2~9 个，中位 4」比「正在以每天 -1.2 下降」诚实得多。
//!
//! 界面上那个瞬时 ETA（librqbit 给的）在这种场景下也没用：一个任务可能
//! 4 MB/s 冲十分钟然后停三小时，瞬时速度算出来的「还剩 8 分钟」是假的。

use serde::Serialize;

use crate::health::Sample;

/// 算实测速度用多长的窗口。太短会被一次爆发带偏，太长又跟不上变化。
const RATE_WINDOW_SECS: i64 = 24 * 3600;

/// 至少要两个带进度的样本才谈得上速度。
const MIN_SAMPLES: usize = 2;

/// 窗口跨度至少这么长，否则算出来的速度没有代表性。
const MIN_SPAN_SECS: i64 = 20 * 60;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Forecast {
    /// 实测平均速度（字节/秒）。**不是瞬时速度**，是窗口内的长期平均。
    pub observed_bps: Option<f64>,
    /// 算这个速度用了多长的窗口（小时）。必须显示出来 —— 「按过去 3 小时」
    /// 和「按过去 24 小时」是两个可信度完全不同的说法。
    pub window_hours: Option<f64>,
    /// 按实测速度还要多久（秒）。
    pub eta_secs: Option<f64>,
    /// 进度已经多久没动了（秒）。None = 一直在动，或者数据不够。
    pub stalled_secs: Option<f64>,
    /// 窗口内做种数的中位数 / 最小 / 最大。**不给趋势外推**，见模块文档。
    pub seeders_median: Option<u32>,
    pub seeders_min: Option<u32>,
    pub seeders_max: Option<u32>,
    /// 一句话。措辞在 Rust 里定死。
    pub summary: String,
}

/// 只留下带进度数据的样本。老样本没有这个字段。
fn with_progress(history: &[Sample]) -> Vec<(i64, u64)> {
    history
        .iter()
        .filter_map(|s| s.progress.map(|p| (s.ts, p)))
        .collect()
}

/// 实测速度：取窗口内最早和最晚两点的差。
///
/// 用端点差而不是逐段平均：**中间停摆的时间必须算进去**。一个任务前 10 分钟
/// 冲了 2 GB、后 20 小时纹丝不动，端点差给出的是真实的「过去 20 小时的平均」，
/// 而逐段平均会把停摆那些 0 速段稀释掉，报出一个乐观得多的数字。
fn observed_rate(points: &[(i64, u64)], now: i64) -> Option<(f64, f64)> {
    if points.len() < MIN_SAMPLES {
        return None;
    }
    let cutoff = now - RATE_WINDOW_SECS;
    let window: Vec<&(i64, u64)> = points.iter().filter(|(t, _)| *t >= cutoff).collect();
    // 窗口内点不够就退回全部历史 —— 有总比没有强，但要如实报窗口长度。
    let window: Vec<&(i64, u64)> = if window.len() >= MIN_SAMPLES {
        window
    } else {
        points.iter().collect()
    };

    let (t0, p0) = **window.first()?;
    let (t1, p1) = **window.last()?;
    let span = t1 - t0;
    if span < MIN_SPAN_SECS {
        return None;
    }
    // 进度会变小：用户改了文件选择，或者重新校验。这时候速度算不了。
    let gained = p1.checked_sub(p0)?;
    Some((gained as f64 / span as f64, span as f64 / 3600.0))
}

/// 进度上次变化到现在多久了。
fn stalled_for(points: &[(i64, u64)], now: i64) -> Option<f64> {
    let last = points.last()?;
    // 从后往前找第一个和最新值不同的点，它之后就一直没动。
    let changed_at = points
        .iter()
        .rev()
        .find(|(_, p)| *p != last.1)
        .map(|(t, _)| *t);
    match changed_at {
        Some(t) => Some((now - t) as f64),
        // 整段历史里进度就没变过。那就从有记录起算。
        None if points.len() >= MIN_SAMPLES => Some((now - points.first()?.0) as f64),
        None => None,
    }
}

fn median(mut v: Vec<u32>) -> Option<u32> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

fn human_duration(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 90.0 {
        format!("{:.0} 秒", s)
    } else if s < 90.0 * 60.0 {
        format!("{:.0} 分钟", s / 60.0)
    } else if s < 48.0 * 3600.0 {
        format!("{:.1} 小时", s / 3600.0)
    } else {
        format!("{:.1} 天", s / 86400.0)
    }
}

fn human_rate(bps: f64) -> String {
    if bps >= 1e6 {
        format!("{:.1} MB/s", bps / 1e6)
    } else if bps >= 1e3 {
        format!("{:.0} KB/s", bps / 1e3)
    } else {
        format!("{:.0} B/s", bps)
    }
}

/// 主入口。`remaining` 是还差多少字节；已完成的任务传 0。
pub fn forecast(history: &[Sample], remaining: u64, now: i64) -> Forecast {
    let points = with_progress(history);

    let valid_seeders: Vec<u32> = history
        .iter()
        .filter(|s| s.trackers_ok > 0)
        .map(|s| s.seeders)
        .collect();

    let mut f = Forecast {
        seeders_median: median(valid_seeders.clone()),
        seeders_min: valid_seeders.iter().copied().min(),
        seeders_max: valid_seeders.iter().copied().max(),
        ..Default::default()
    };

    if remaining == 0 {
        f.summary = "已经下完了".into();
        return f;
    }

    if points.len() < MIN_SAMPLES {
        f.summary = "还在攒数据 —— 至少要两轮采样才能算出实测速度".into();
        return f;
    }

    f.stalled_secs = stalled_for(&points, now);

    match observed_rate(&points, now) {
        Some((bps, hours)) if bps > 0.0 => {
            f.observed_bps = Some(bps);
            f.window_hours = Some(hours);
            let eta = remaining as f64 / bps;
            f.eta_secs = Some(eta);
            f.summary = format!(
                "按过去 {:.0} 小时的实测速度（{}），还要 {}",
                hours,
                human_rate(bps),
                human_duration(eta)
            );
        }
        Some((_, hours)) => {
            // 窗口内一个字节都没涨。
            f.window_hours = Some(hours);
            f.observed_bps = Some(0.0);
            f.summary = match f.stalled_secs {
                Some(s) => format!(
                    "过去 {:.0} 小时一个字节都没下到（已经卡了 {}）—— 给不出剩余时间",
                    hours,
                    human_duration(s)
                ),
                None => format!("过去 {hours:.0} 小时一个字节都没下到 —— 给不出剩余时间"),
            };
        }
        None => {
            f.summary = "采样跨度还不够，算不出有代表性的速度".into();
        }
    }

    f
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;

    fn s(ts: i64, seeders: u32, progress: Option<u64>) -> Sample {
        Sample {
            ts,
            seeders,
            leechers: 1,
            trackers_ok: 2,
            progress,
            total: Some(100_000_000_000),
        }
    }

    fn gb(n: f64) -> u64 {
        (n * 1e9) as u64
    }

    #[test]
    fn not_enough_data_says_so() {
        let f = forecast(&[s(0, 5, Some(0))], gb(10.0), 3600);
        assert!(f.eta_secs.is_none());
        assert!(f.summary.contains("攒数据"), "实际：{}", f.summary);
    }

    /// 老样本没有 progress 字段。不能因为它们存在就以为有数据。
    #[test]
    fn samples_without_progress_are_ignored() {
        let h: Vec<Sample> = (0..20).map(|i| s(i * H, 5, None)).collect();
        let f = forecast(&h, gb(10.0), 20 * H);
        assert!(f.eta_secs.is_none());
        assert!(f.summary.contains("攒数据"), "实际：{}", f.summary);
        // 但做种数的统计照样该有 —— 那个字段老样本里是有的。
        assert_eq!(f.seeders_median, Some(5));
    }

    #[test]
    fn computes_rate_and_eta() {
        // 10 小时下了 10 GB = 约 278 KB/s，还剩 20 GB → 约 20 小时
        let h: Vec<Sample> = (0..=10).map(|i| s(i * H, 5, Some(gb(i as f64)))).collect();
        let f = forecast(&h, gb(20.0), 10 * H);
        let bps = f.observed_bps.unwrap();
        assert!((bps - gb(1.0) as f64 / H as f64).abs() < 1.0, "速度算错：{bps}");
        let eta = f.eta_secs.unwrap();
        assert!((eta - 20.0 * H as f64).abs() < 60.0, "ETA 算错：{eta}");
        assert!(f.summary.contains("还要"), "实际：{}", f.summary);
    }

    /// 这条是这个模块的核心取舍。
    ///
    /// 任务前 30 分钟冲了 5 GB，之后 20 小时纹丝不动。逐段平均会把停摆的
    /// 0 速段稀释掉、报出一个乐观得多的数字；端点差如实反映「过去 20 小时
    /// 基本没动」。用户要拿这个数字决定等不等，不能给他一个假的希望。
    #[test]
    fn burst_then_stall_reports_the_real_average() {
        let mut h = vec![s(0, 5, Some(0)), s(1800, 5, Some(gb(5.0)))];
        for i in 1..=20 {
            h.push(s(1800 + i * H, 5, Some(gb(5.0))));
        }
        let now = 1800 + 20 * H;
        let f = forecast(&h, gb(15.0), now);

        let bps = f.observed_bps.unwrap();
        // 真实平均 ≈ 5GB / 20.5h ≈ 68 KB/s，远低于爆发时的 2.9 MB/s
        assert!(bps < 100_000.0, "把爆发速度当成了平均：{}", human_rate(bps));
        // 而且要说出卡了多久
        let stalled = f.stalled_secs.unwrap();
        assert!(stalled >= 19.0 * H as f64, "没算对停摆时长：{stalled}");
    }

    #[test]
    fn fully_stalled_refuses_to_give_eta() {
        let h: Vec<Sample> = (0..=30).map(|i| s(i * H, 3, Some(gb(4.0)))).collect();
        let f = forecast(&h, gb(10.0), 30 * H);
        assert!(f.eta_secs.is_none(), "卡死了不该给剩余时间");
        assert!(f.summary.contains("没下到"), "实际：{}", f.summary);
        assert!(f.stalled_secs.unwrap() >= 29.0 * H as f64);
    }

    /// 改了文件选择或者重新校验时进度会变小。这时候速度算不了，
    /// 不能返回一个负数或者巨大的 ETA。
    #[test]
    fn progress_going_backwards_is_survivable() {
        let h = vec![
            s(0, 5, Some(gb(10.0))),
            s(H, 5, Some(gb(11.0))),
            s(2 * H, 5, Some(gb(2.0))), // 用户取消了几个文件
        ];
        let f = forecast(&h, gb(5.0), 2 * H);
        assert!(f.eta_secs.is_none(), "进度倒退时不该给 ETA");
        assert!(f.observed_bps.is_none());
    }

    /// 跨度太短算出来的速度没代表性 —— 采样间隔是 30 分钟，两个挨着的
    /// 样本之间可能刚好是一次爆发。
    #[test]
    fn too_short_a_span_is_refused() {
        let h = vec![s(0, 5, Some(0)), s(300, 5, Some(gb(1.0)))];
        let f = forecast(&h, gb(10.0), 300);
        assert!(f.eta_secs.is_none());
        assert!(f.summary.contains("跨度"), "实际：{}", f.summary);
    }

    /// 报做种数的区间和中位数，而不是拟合趋势线 —— 见模块文档里那组
    /// 变异系数。
    #[test]
    fn reports_seeder_range_not_a_trend() {
        let counts = [5u32, 5, 8, 7, 7, 12, 11, 8, 9, 7, 7, 6];
        let h: Vec<Sample> = counts
            .iter()
            .enumerate()
            .map(|(i, c)| s(i as i64 * H, *c, Some(gb(i as f64))))
            .collect();
        let f = forecast(&h, gb(5.0), 11 * H);
        assert_eq!(f.seeders_min, Some(5));
        assert_eq!(f.seeders_max, Some(12));
        assert_eq!(f.seeders_median, Some(7));
        // 不该出现任何「多少天后归零」之类的外推
        assert!(!f.summary.contains("归零"));
        assert!(!f.summary.contains("概率"));
    }

    #[test]
    fn finished_torrent_says_so() {
        let h: Vec<Sample> = (0..=5).map(|i| s(i * H, 5, Some(gb(10.0)))).collect();
        let f = forecast(&h, 0, 5 * H);
        assert!(f.summary.contains("下完"), "实际：{}", f.summary);
        assert!(f.eta_secs.is_none());
    }

    /// 窗口长度必须报出来 —— 「按过去 1 小时」和「按过去 24 小时」的可信度
    /// 完全不同，不说等于让人自己脑补。
    #[test]
    fn window_length_is_always_reported() {
        let h: Vec<Sample> = (0..=10).map(|i| s(i * H, 5, Some(gb(i as f64)))).collect();
        let f = forecast(&h, gb(20.0), 10 * H);
        assert!(f.window_hours.is_some());
        assert!(f.summary.contains("小时的实测速度"), "实际：{}", f.summary);
    }

    #[test]
    fn formats_durations_sensibly() {
        assert_eq!(human_duration(45.0), "45 秒");
        assert_eq!(human_duration(600.0), "10 分钟");
        assert_eq!(human_duration(7200.0), "2.0 小时");
        assert_eq!(human_duration(3.0 * 86400.0), "3.0 天");
    }
}
