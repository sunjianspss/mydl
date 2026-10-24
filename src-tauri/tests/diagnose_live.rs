//! 真机上跑一次完整诊断，把阶梯和结论打出来。
//!
//! ```
//! cargo test --test diagnose_live -- --ignored --nocapture
//! ```
//!
//! 不断言结论内容 —— 那取决于跑的时候网络和 swarm 什么样。只保证整条链路
//! 能跑通、不 panic、每一步都有结果。
use mydl_lib::diagnose;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，会连 tracker 和若干 peer，最长约 40 秒"]
async fn runs_end_to_end() {
    let info_hash = std::env::var("MYDL_DIAG_HASH")
        .unwrap_or_else(|_| "a57b7f3548bcab81116f5bd282dce952b4215853".into());
    let bind = std::env::var("MYDL_DIAG_BIND").ok();

    eprintln!("info_hash={info_hash}  绑定网卡={bind:?}");
    let t0 = std::time::Instant::now();
    let r = diagnose::run(
        &info_hash,
        diagnose::Subject {
            state: "live".into(),
            bind_device: bind,
            ..Default::default()
        },
    )
    .await;
    eprintln!("用时 {:.1}s\n", t0.elapsed().as_secs_f64());

    for s in &r.steps {
        eprintln!("  [{:?}] {:<24} {}", s.outcome, s.name, s.detail);
    }
    eprintln!("\n结论：{}", r.verdict);
    if let Some(a) = &r.advice {
        eprintln!("建议：{a}");
    }

    assert!(!r.steps.is_empty(), "一步都没跑");
    assert!(!r.verdict.is_empty(), "没给出结论");
}

/// 单独跑对照组：开隔离实例真下一小会儿 Ubuntu 官方种子。
///
/// ```
/// MYDL_DIAG_BIND=en0 cargo test --test diagnose_live control -- --ignored --nocapture
/// ```
///
/// 不断言下了多少 —— 那是网络测量。只保证它真的跑起来了、跑完把临时目录
/// 删干净了。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，会实际下载十几 MB，最长约 40 秒"]
async fn control_download_runs() {
    let bind = std::env::var("MYDL_DIAG_BIND").ok();
    let before = leftover_dirs();

    let t0 = std::time::Instant::now();
    let c = diagnose::control_download(bind.as_deref()).await;
    eprintln!("绑定网卡={bind:?}  用时 {:.1}s  {c:?}", t0.elapsed().as_secs_f64());

    assert!(c.is_some(), "对照组没跑起来");
    assert_eq!(leftover_dirs(), before, "临时目录没删干净");
}

fn leftover_dirs() -> usize {
    std::fs::read_dir(std::env::temp_dir())
        .map(|it| {
            it.filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("mydl-diag-control-"))
                .count()
        })
        .unwrap_or(0)
}
