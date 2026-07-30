//! 本地流媒体服务：把 librqbit 的 `FileStream` 包成支持 HTTP Range 的端点，
//! 播放器（IINA / VLC / QuickTime）就能直接边下边播，还能任意拖进度。
//!
//! librqbit 在 `stream()` 里会把该文件的分片提到最高优先级，并按
//! 「首片 → 尾片 → 中间顺序」的次序抓，所以拖动和起播都不用等整个种子下完。
//!
//! 安全上做两件事：只绑 127.0.0.1，且每次启动生成一次性 token 放在路径里，
//! 免得同机其他程序能随意枚举、读取正在下载的内容。

use std::io::SeekFrom;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::engine::Engine;

pub struct StreamServer {
    port: u16,
    token: String,
}

struct ServerState {
    engine: Arc<Engine>,
    token: String,
}

impl StreamServer {
    pub async fn start(engine: Arc<Engine>) -> Result<Self> {
        let token = format!("{:032x}", rand::random::<u128>());
        let state = Arc::new(ServerState {
            engine,
            token: token.clone(),
        });

        // 末尾的文件名段服务端不用，纯粹是让播放器能从扩展名认出容器格式。
        let app = Router::new()
            .route("/s/{token}/{torrent_id}/{file_id}/{*name}", get(serve))
            .with_state(state);

        // 端口交给系统分配，避免和别的程序抢固定端口。
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .context("绑定本地流媒体端口失败")?;
        let port = listener.local_addr()?.port();

        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("流媒体服务退出：{e:#}");
            }
        });

        tracing::info!("流媒体服务监听 127.0.0.1:{port}");
        Ok(Self { port, token })
    }

    pub fn url_for(&self, torrent_id: usize, file_id: usize, filename: &str) -> String {
        format!(
            "http://127.0.0.1:{}/s/{}/{}/{}/{}",
            self.port,
            self.token,
            torrent_id,
            file_id,
            url_safe_name(filename)
        )
    }
}

async fn serve(
    State(state): State<Arc<ServerState>>,
    Path((token, torrent_id, file_id, _name)): Path<(String, usize, usize, String)>,
    headers: HeaderMap,
) -> Response {
    if token != state.token {
        return StatusCode::FORBIDDEN.into_response();
    }

    // 文件长度和真实文件名都从元信息拿：FileStream 的类型不可命名，
    // 也就用不了它的 len()。顺便用真实文件名判断 Content-Type，比 URL 末段可靠。
    let file = match state.engine.files(torrent_id) {
        Ok(files) => match files.into_iter().nth(file_id) {
            Some(f) => f,
            None => return (StatusCode::NOT_FOUND, "文件不存在").into_response(),
        },
        Err(e) => return (StatusCode::NOT_FOUND, format!("{e:#}")).into_response(),
    };

    let total = file.len;
    if total == 0 {
        return StatusCode::NO_CONTENT.into_response();
    }

    let mut stream = match state.engine.open_stream(torrent_id, file_id).await {
        Ok(s) => s,
        Err(e) => return (StatusCode::NOT_FOUND, format!("{e:#}")).into_response(),
    };

    let requested = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, total));

    let (start, end, status) = match requested {
        // Range 头存在但解析不出来 / 越界，必须回 416，否则播放器会拿到错位的数据。
        Some(None) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            )
                .into_response()
        }
        Some(Some((s, e))) => (s, e, StatusCode::PARTIAL_CONTENT),
        None => (0, total - 1, StatusCode::OK),
    };

    if let Err(e) = stream.seek(SeekFrom::Start(start)).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    let length = end - start + 1;

    // 播放器的行为只能从这里观察：起播、拖进度、反复重连都长得不一样。
    tracing::info!(
        torrent = torrent_id,
        file = file_id,
        name = %file.name,
        range = %format!("{start}-{end}/{total}"),
        status = status.as_u16(),
        "流请求"
    );

    let body = Body::from_stream(ReaderStream::new(stream.take(length)));

    let mut resp = Response::builder()
        .status(status)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length)
        .header(header::CONTENT_TYPE, mime_for(&file.name));

    if status == StatusCode::PARTIAL_CONTENT {
        resp = resp.header(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).unwrap(),
        );
    }

    resp.body(body)
        .unwrap_or_else(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())
}

/// 解析单段 Range，返回闭区间 [start, end]。
///
/// 外层 `Option` 区分「没有 Range 头」和「有但不合法」，后者要回 416。
/// 多段 Range（`bytes=0-10,20-30`）直接当不合法处理 —— 播放器不会用。
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;

    let (start, end) = match (start.trim(), end.trim()) {
        // bytes=-500 表示最后 500 字节
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            if n == 0 {
                return None;
            }
            (total.saturating_sub(n), total - 1)
        }
        // bytes=500-
        (s, "") => (s.parse().ok()?, total - 1),
        // bytes=500-999
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(total - 1)),
    };

    if start > end || start >= total {
        return None;
    }
    Some((start, end))
}

fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" | "m2ts" => "video/mp2t",
        "flv" => "video/x-flv",
        "wmv" => "video/x-ms-wmv",
        "ogv" => "video/ogg",
        "mpg" | "mpeg" | "m2v" => "video/mpeg",
        "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "wma" => "audio/x-ms-wma",
        "ogg" | "opus" => "audio/ogg",
        "srt" => "application/x-subrip",
        "ass" | "ssa" => "text/x-ssa",
        _ => "application/octet-stream",
    }
}

/// URL 末段只是给播放器看扩展名用的，服务端忽略，所以直接把可能引起歧义的
/// 字符替换掉，比做完整的百分号编码更省事也更不容易出错。
fn url_safe_name(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_range_forms() {
        assert_eq!(parse_range("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-500", 1000), Some((500, 999)));
        // 末尾越界要夹到文件末尾，播放器常这么发
        assert_eq!(parse_range("bytes=0-99999", 1000), Some((0, 999)));
    }

    #[test]
    fn rejects_bad_ranges() {
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=600-500", 1000), None);
        assert_eq!(parse_range("bytes=0-10,20-30", 1000), None);
        assert_eq!(parse_range("items=0-10", 1000), None);
        assert_eq!(parse_range("bytes=abc-def", 1000), None);
    }

    #[test]
    fn sanitizes_url_names() {
        // 只保留基础名，空格和括号各自换成一个下划线
        assert_eq!(
            url_safe_name("Some Show/S01E01 [1080p].mkv"),
            "S01E01__1080p_.mkv"
        );
        assert_eq!(url_safe_name(""), "file");
    }
}
