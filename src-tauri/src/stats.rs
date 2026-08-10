//! 长期统计：每天下载/上传了多少。
//!
//! # 为什么要单独存
//!
//! 这个系统之前**从来没记过长期数据**。`health.rs` 的采样只留最近 480 条
//! （约 10 天），而且是按种子存的；librqbit 的 `uploaded_bytes` 干脆不持久化，
//! 每次重启归零。想要一张跨月的活动图，只能自己按天攒。
//!
//! 一天一条记录，一年 365 条，几十 KB —— 比按种子留原始采样省得多。
//!
//! # 增量怎么算才不会算错
//!
//! 不能拿任务的 `progress_bytes` 记流量。它表示「当前选中文件里磁盘上已经有
//! 多少」，App 启动校验、重新选择文件时都会大幅回退再恢复；把恢复量当下载量
//! 会让同一批旧文件被重复累计。
//!
//! librqbit 的会话级 `fetched_bytes` / `uploaded_bytes` 才是真正的 BT 传输计数器。
//! 它们在一个 App 进程内单调递增，重启后归零。每轮只记相对上一轮的增量；
//! 本进程的第一轮则把当前值全部记下，因为这些字节都发生在 App 启动之后、
//! 第一次采样之前。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 热力图最多回看多少天。半年，够看出规律又不至于把格子压得看不清。
pub const HEATMAP_DAYS: i64 = 182;

/// 一天的总量。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Day {
    pub down: u64,
    pub up: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct StatsData {
    /// `YYYY-MM-DD` -> 当天总量。
    days: HashMap<String, Day>,
    /// 开始统计的日期。界面上要显示 —— 「累计」只能从这天算起，
    /// 说成「历史总量」是撒谎。
    since: Option<String>,
    /// 本进程上一轮看到的会话级 (已下载, 已上传)。会话计数器随进程归零，
    /// 所以这个基线绝不能持久化。旧版 JSON 里的 `last` 字段会由 serde 忽略。
    #[serde(skip)]
    session_last: Option<(u64, u64)>,
}

pub struct StatsStore {
    path: PathBuf,
    data: Mutex<StatsData>,
}

/// Unix 秒 -> 本地日期 `YYYY-MM-DD`。
///
/// 自己算而不是引 chrono：只需要这一个换算，而且**要用本地时区** ——
/// 按 UTC 分天的话，东八区的用户会看到活动被劈到前一天的深夜。
fn local_date(ts: i64) -> String {
    // std 拿不到时区偏移，用 localtime 和 gmtime 的差反推。够用且不引依赖。
    let offset = local_offset_secs();
    let days = (ts + offset).div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 本地时区相对 UTC 的偏移（秒）。缓存一次 —— 进程生命周期内不会变，
/// 而夏令时切换那一天差一小时不影响按天分桶。
fn local_offset_secs() -> i64 {
    use std::sync::OnceLock;
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        // 拿系统的 `date +%z`，比自己 FFI 调 localtime_r 简单且不引 libc。
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| parse_utc_offset(s.trim()))
            .unwrap_or(0)
    })
}

/// 解析 `+0800` / `-0500` 这种偏移。
fn parse_utc_offset(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 5 || (b[0] != b'+' && b[0] != b'-') {
        return None;
    }
    let h: i64 = s.get(1..3)?.parse().ok()?;
    let m: i64 = s.get(3..5)?.parse().ok()?;
    let mag = h * 3600 + m * 60;
    Some(if b[0] == b'-' { -mag } else { mag })
}

/// 从「1970-01-01 起的天数」还原年月日。Howard Hinnant 的 civil_from_days。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl StatsStore {
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            data: Mutex::new(data),
        }
    }

    /// 记一轮。`current` 是 librqbit 本次会话累计的 (已下载, 已上传)。
    ///
    /// 返回这轮记了多少新增字节 (下, 上)，主要给日志和测试看。
    pub fn record(&self, current: (u64, u64), now_ts: i64) -> (u64, u64) {
        let date = local_date(now_ts);
        let mut data = self.data.lock().unwrap();

        if data.since.is_none() {
            data.since = Some(date.clone());
        }

        let (down, up) = match data.session_last.replace(current) {
            Some(last) => (
                session_counter_delta(current.0, last.0),
                session_counter_delta(current.1, last.1),
            ),
            // 计数器从 App 启动时的 0 开始；第一轮已有的值也是本次运行期间
            // 真正传输的流量，不能只拿来做基线而丢掉。
            None => current,
        };

        let entry = data.days.entry(date).or_default();
        entry.down += down;
        entry.up += up;

        // 一年以上的就不留了。
        let cutoff = local_date(now_ts - 400 * 86400);
        data.days.retain(|d, _| d.as_str() >= cutoff.as_str());

        drop(data);
        self.save();
        (down, up)
    }

    pub fn snapshot(&self, now_ts: i64) -> Report {
        let data = self.data.lock().unwrap();
        build_report(&data.days, data.since.as_deref(), now_ts)
    }

    fn save(&self) {
        let data = self.data.lock().unwrap();
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&*data) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// 会话计数器正常情况下单调递增。若底层会话在进程内被重建而归零，
/// 当前值就是新会话已经产生的全部流量，也应该记入而不是丢掉。
fn session_counter_delta(current: u64, last: u64) -> u64 {
    current.checked_sub(last).unwrap_or(current)
}

pub fn stats_path(config_dir: &Path) -> PathBuf {
    config_dir.join("daily_stats.json")
}

// ---------------------------------------------------------------------------
// 汇总
// ---------------------------------------------------------------------------

/// 热力图里的一格。
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub date: String,
    pub down: u64,
    pub up: u64,
    /// 这一天在不在统计范围内。**开始统计之前的日子是「没有数据」，
    /// 不是「那天没下载」** —— 两者在图上必须长得不一样。
    pub tracked: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// 从哪天开始统计的。累计值只能从这天算起。
    pub since: Option<String>,
    pub total_down: u64,
    pub total_up: u64,
    /// 单日最高下载量，以及是哪天。
    pub peak_down: u64,
    pub peak_date: Option<String>,
    /// 当前连续有下载的天数（截至今天或昨天）。
    pub current_streak: u32,
    pub longest_streak: u32,
    /// 有过下载的天数。
    pub active_days: u32,
    /// 按日期升序，最后一格是今天。
    pub cells: Vec<Cell>,
}

fn day_before(date: &str, n: i64) -> String {
    // 反解 YYYY-MM-DD 再减天数。只在汇总时调，性能无所谓。
    let parts: Vec<i64> = date.split('-').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 3 {
        return date.to_string();
    }
    let days = days_from_civil(parts[0], parts[1] as u32, parts[2] as u32) - n;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 纯函数，好测。
pub fn build_report(days: &HashMap<String, Day>, since: Option<&str>, now_ts: i64) -> Report {
    let today = local_date(now_ts);

    let cells: Vec<Cell> = (0..HEATMAP_DAYS)
        .rev()
        .map(|i| {
            let date = day_before(&today, i);
            let d = days.get(&date).copied().unwrap_or_default();
            Cell {
                tracked: since.is_some_and(|s| date.as_str() >= s),
                date,
                down: d.down,
                up: d.up,
            }
        })
        .collect();

    let total_down = days.values().map(|d| d.down).sum();
    let total_up = days.values().map(|d| d.up).sum();

    let (peak_date, peak_down) = days
        .iter()
        .filter(|(_, d)| d.down > 0)
        .max_by_key(|(_, d)| d.down)
        .map(|(k, d)| (Some(k.clone()), d.down))
        .unwrap_or((None, 0));

    // 连续天数按「有下载」算。用 cells 的顺序，天然按日期升序且没有空洞。
    let active: Vec<bool> = cells.iter().map(|c| c.down > 0).collect();
    let mut longest = 0u32;
    let mut run = 0u32;
    for a in &active {
        run = if *a { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    // 当前连续：从末尾往前数。**今天还没下载不该把连续记录清零** ——
    // 一天才刚开始。所以允许最后一天是空的，从倒数第二天接着数。
    let mut current = 0u32;
    let mut it = active.iter().rev();
    if let Some(false) = it.next() {
        // 今天还没有，从昨天开始数
    } else {
        current = 1;
    }
    for a in it {
        if *a {
            current += 1;
        } else {
            break;
        }
    }

    Report {
        since: since.map(str::to_string),
        total_down,
        total_up,
        peak_down,
        peak_date,
        current_streak: current,
        longest_streak: longest,
        active_days: active.iter().filter(|a| **a).count() as u32,
        cells,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (StatsStore, PathBuf) {
        let p = std::env::temp_dir().join(format!("mydl-stats-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        (StatsStore::load(p.clone()), p)
    }

    /// 2026-08-06 12:00 UTC 附近的时间戳，用来对日期换算。
    const T: i64 = 1_785_974_400;

    #[test]
    fn converts_days_both_ways() {
        for (y, m, d) in [(1970, 1, 1), (2000, 2, 29), (2026, 8, 6), (2026, 12, 31)] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d), "{y}-{m}-{d} 换算不闭合");
        }
    }

    #[test]
    fn parses_utc_offsets() {
        assert_eq!(parse_utc_offset("+0800"), Some(8 * 3600));
        assert_eq!(parse_utc_offset("-0500"), Some(-5 * 3600));
        assert_eq!(parse_utc_offset("+0530"), Some(5 * 3600 + 1800));
        assert_eq!(parse_utc_offset("+0000"), Some(0));
        assert_eq!(parse_utc_offset("garbage"), None);
    }

    /// 会话计数器从 App 启动时的 0 开始，第一次采样前的流量也不能漏。
    #[test]
    fn first_round_records_traffic_since_app_start() {
        let (s, p) = store("first");
        let (down, up) = s.record((2_000, 500), T);
        assert_eq!((down, up), (2_000, 500));
        assert_eq!(s.snapshot(T).total_down, 2_000);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn accumulates_deltas() {
        let (s, p) = store("delta");
        s.record((1_000, 10), T);
        let (d, u) = s.record((3_000, 30), T);
        assert_eq!((d, u), (2000, 20));
        assert_eq!(s.snapshot(T).total_down, 3_000);
        let _ = std::fs::remove_file(p);
    }

    /// 底层会话若在进程内重建，计数器会归零。新会话当前已有的流量
    /// 应该完整计入，不能等它重新追上旧基线。
    #[test]
    fn counter_reset_starts_a_new_epoch() {
        let (s, p) = store("reset");
        s.record((10_000, 100), T);

        let (d, u) = s.record((2_000, 20), T);
        assert_eq!((d, u), (2_000, 20));

        let (d2, u2) = s.record((2_500, 35), T);
        assert_eq!((d2, u2), (500, 15));
        assert_eq!(s.snapshot(T).total_down, 12_500);
        let _ = std::fs::remove_file(p);
    }

    /// App 重启会创建新的 StatsStore 和新的 librqbit 会话。旧累计量保留，
    /// 新会话第一次采样的字节应作为新的流量追加。
    #[test]
    fn app_restart_keeps_totals_and_counts_the_new_session() {
        let (s, p) = store("restart");
        s.record((5_000, 200), T);
        drop(s);

        let reopened = StatsStore::load(p.clone());
        let (d, u) = reopened.record((1_000, 30), T);
        assert_eq!((d, u), (1_000, 30));
        assert_eq!(reopened.snapshot(T).total_down, 6_000);
        let _ = std::fs::remove_file(p);
    }

    /// v0.14.1 把每个种子的磁盘进度基线存在 `last`。升级后要能读旧文件，
    /// 但下一次保存应丢掉这个已经没有意义、还会诱发重复计数的字段。
    #[test]
    fn loads_and_migrates_legacy_progress_baselines() {
        let (_, p) = store("legacy");
        std::fs::write(
            &p,
            r#"{"days":{"2026-08-06":{"down":123,"up":45}},"last":{"hash":[50000000000,9]},"since":"2026-08-06"}"#,
        )
        .unwrap();

        let s = StatsStore::load(p.clone());
        assert_eq!(s.snapshot(T).total_down, 123);
        assert_eq!(s.record((0, 0), T), (0, 0));

        let saved = std::fs::read_to_string(&p).unwrap();
        assert!(!saved.contains("\"last\""), "旧进度基线不该继续保存");
        let _ = std::fs::remove_file(p);
    }

    /// 复现这次 89 GB 的根因：启动校验让磁盘进度从低位恢复到几十 GB，
    /// 但会话传输计数器没变，所以统计必须仍为 0。
    #[test]
    fn startup_disk_check_does_not_create_download_traffic() {
        let (s, p) = store("startup-check");
        s.record((0, 0), T); // 校验中，任务进度很低
        let (d, u) = s.record((0, 0), T); // 校验完成，任务进度恢复
        assert_eq!((d, u), (0, 0));
        assert_eq!(s.snapshot(T).total_down, 0);

        let (d2, _) = s.record((128 * 1024, 0), T);
        assert_eq!(d2, 128 * 1024, "真正收到的 BT payload 才能进入统计");
        let _ = std::fs::remove_file(p);
    }

    // ---- 汇总 ----

    fn days_map(pairs: &[(&str, u64)]) -> HashMap<String, Day> {
        pairs
            .iter()
            .map(|(d, v)| ((*d).into(), Day { down: *v, up: 0 }))
            .collect()
    }

    #[test]
    fn cells_cover_the_window_and_end_today() {
        let r = build_report(&HashMap::new(), Some("2026-01-01"), T);
        assert_eq!(r.cells.len() as i64, HEATMAP_DAYS);
        assert_eq!(r.cells.last().unwrap().date, local_date(T));
    }

    /// 开始统计之前的日子是「没有数据」，不是「那天没下载」。
    /// 图上必须能区分，否则新装的用户会看到半年的「零活动」。
    #[test]
    fn days_before_tracking_are_marked_untracked() {
        let today = local_date(T);
        let r = build_report(&HashMap::new(), Some(&today), T);
        assert!(r.cells.last().unwrap().tracked);
        assert!(!r.cells.first().unwrap().tracked, "半年前不该算「有统计」");
    }

    #[test]
    fn computes_peak_and_active_days() {
        let today = local_date(T);
        let m = days_map(&[
            (&day_before(&today, 1), 500),
            (&day_before(&today, 2), 9000),
            (&day_before(&today, 3), 100),
        ]);
        let r = build_report(&m, Some("2026-01-01"), T);
        assert_eq!(r.peak_down, 9000);
        assert_eq!(r.peak_date.as_deref(), Some(day_before(&today, 2).as_str()));
        assert_eq!(r.active_days, 3);
        assert_eq!(r.total_down, 9600);
    }

    /// 今天才刚开始，还没下载不该把连续记录清零。
    #[test]
    fn today_being_empty_does_not_break_the_streak() {
        let today = local_date(T);
        let m = days_map(&[
            (&day_before(&today, 1), 100),
            (&day_before(&today, 2), 100),
            (&day_before(&today, 3), 100),
        ]);
        let r = build_report(&m, Some("2026-01-01"), T);
        assert_eq!(r.current_streak, 3, "今天空着不该断掉昨天起的连续");
    }

    #[test]
    fn streak_breaks_on_a_real_gap() {
        let today = local_date(T);
        let m = days_map(&[
            (&today, 100),
            (&day_before(&today, 1), 100),
            // 第 2 天空
            (&day_before(&today, 3), 100),
            (&day_before(&today, 4), 100),
            (&day_before(&today, 5), 100),
        ]);
        let r = build_report(&m, Some("2026-01-01"), T);
        assert_eq!(r.current_streak, 2);
        assert_eq!(r.longest_streak, 3);
    }

    #[test]
    fn empty_history_is_all_zero_not_a_panic() {
        let r = build_report(&HashMap::new(), None, T);
        assert_eq!(r.total_down, 0);
        assert_eq!(r.current_streak, 0);
        assert_eq!(r.longest_streak, 0);
        assert!(r.peak_date.is_none());
        assert!(r.cells.iter().all(|c| !c.tracked), "没开始统计时全是未跟踪");
    }
}
