//! 边下边播的端到端测试：真下一个种子，然后像播放器那样发 Range 请求，
//! 验证 206 / Content-Range / 实际字节数都对得上。
//!
//! 默认被 `#[ignore]` 跳过。手动跑：
//!
//! ```
//! cargo test --test stream_smoke -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use mydl_lib::engine::Engine;
use mydl_lib::stream_server::StreamServer;

const TORRENT_URL: &str =
    "https://releases.ubuntu.com/24.04/ubuntu-24.04.4-live-server-amd64.iso.torrent";

const CHUNK: u64 = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(180);

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要外网，会实际下载几 MB 数据"]
async fn serves_range_requests() {
    let tmp = std::env::temp_dir().join(format!("mydl-stream-{}", std::process::id()));
    let state_dir = tmp.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();

    let engine = Arc::new(
        Engine::new(tmp.join("downloads"), Some(state_dir), Vec::new())
            .await
            .expect("创建 Engine 失败"),
    );
    let server = StreamServer::start(engine.clone())
        .await
        .expect("启动流媒体服务失败");

    let id = engine.add(TORRENT_URL, None).await.expect("添加任务失败");

    // 等元信息解析出来，拿到文件列表。
    let started = Instant::now();
    let file = loop {
        if let Some(f) = engine.files(id).expect("读文件列表失败").into_iter().next() {
            break f;
        }
        assert!(started.elapsed() < TIMEOUT, "一直没拿到种子元信息");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    eprintln!("文件：{} ({} bytes)", file.name, file.len);

    let url = server.url_for(id, 0, &file.name);
    eprintln!("流地址：{url}");

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .expect("构建 http client 失败");

    // ---- 1. 播放器起播时的典型请求：要开头一小段 ----
    let end = CHUNK - 1;
    let resp = client
        .get(&url)
        .header("Range", format!("bytes=0-{end}"))
        .send()
        .await
        .expect("Range 请求失败");

    if resp.status().as_u16() != 206 {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        panic!("带 Range 必须回 206，实际 {status}，响应体：{body:?}");
    }
    assert_eq!(
        resp.headers().get("content-range").unwrap().to_str().unwrap(),
        format!("bytes 0-{end}/{}", file.len),
    );
    assert_eq!(
        resp.headers().get("accept-ranges").unwrap().to_str().unwrap(),
        "bytes",
    );

    let body = resp.bytes().await.expect("读取响应体失败");
    assert_eq!(body.len() as u64, CHUNK, "返回的字节数和请求的区间不一致");
    eprintln!("起播分片 OK：{} bytes", body.len());

    // ---- 2. 拖进度：跳到文件中间要一段 ----
    let mid = file.len / 2;
    let mid_end = mid + CHUNK - 1;
    let resp = client
        .get(&url)
        .header("Range", format!("bytes={mid}-{mid_end}"))
        .send()
        .await
        .expect("中段 Range 请求失败");

    assert_eq!(resp.status().as_u16(), 206);
    assert_eq!(
        resp.headers().get("content-range").unwrap().to_str().unwrap(),
        format!("bytes {mid}-{mid_end}/{}", file.len),
    );
    let body = resp.bytes().await.expect("读取中段失败");
    assert_eq!(body.len() as u64, CHUNK, "拖进度后返回的字节数不对");
    eprintln!("拖进度 OK：从 {mid} 取到 {} bytes", body.len());

    // ---- 3. 不带 Range：应回 200 且 Content-Length 是整个文件 ----
    let resp = client.get(&url).send().await.expect("无 Range 请求失败");
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.content_length(), Some(file.len));
    drop(resp); // 别真把整个 ISO 拉下来

    // ---- 4. 越界 Range：必须回 416，不能给错位数据 ----
    let resp = client
        .get(&url)
        .header("Range", format!("bytes={}-", file.len + 1))
        .send()
        .await
        .expect("越界 Range 请求失败");
    assert_eq!(resp.status().as_u16(), 416);

    // ---- 5. token 不对：403 ----
    let bad = url.replace("/s/", "/s/x");
    let resp = client.get(&bad).send().await.expect("错误 token 请求失败");
    assert!(
        resp.status().as_u16() == 403 || resp.status().as_u16() == 404,
        "错误 token 不该拿到内容，实际 {}",
        resp.status()
    );

    engine.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("全部通过");
}
