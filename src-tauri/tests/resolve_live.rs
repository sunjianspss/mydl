//! 用真实的 Jackett 302 验 resolve_uri。
//!
//! 需要本机 Jackett 在跑，且地址里带 apikey —— **不能写死在文件里**，
//! 所以从环境变量取，没设就跳过。手动跑：
//!
//! ```
//! MYDL_DL_URL='http://127.0.0.1:9117/dl/limetorrents/?jackett_apikey=…&path=…' \
//!   cargo test --test resolve_live -- --ignored --nocapture
//! ```

use mydl_lib::search::resolve_uri;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要本机 Jackett，且要用 MYDL_DL_URL 指定一条 /dl/ 地址"]
async fn resolves_jackett_redirect_to_magnet() {
    let Ok(dl) = std::env::var("MYDL_DL_URL") else {
        eprintln!("没设 MYDL_DL_URL，跳过");
        return;
    };

    let out = resolve_uri(&dl).await;
    eprintln!("原始: {}…", &dl[..70.min(dl.len())]);
    eprintln!("解析: {}…", &out[..70.min(out.len())]);
    assert!(
        out.starts_with("magnet:?xt=urn:btih:"),
        "Jackett 的 /dl/ 对只给磁力链的站会 302，应该被解成磁力链，实际：{out}"
    );
}

/// 非 http 的原样返回，一个请求都不该发。
#[tokio::test(flavor = "multi_thread")]
async fn passes_through_non_http() {
    let m = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
    assert_eq!(resolve_uri(m).await, m);
    assert_eq!(resolve_uri("/tmp/a.torrent").await, "/tmp/a.torrent");
}
