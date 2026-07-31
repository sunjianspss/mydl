/**
 * 把平台写到 <html data-platform>，CSS 据此决定要不要留标题栏空白。
 *
 * 用 userAgent 而不是调 Rust 命令：这个判断必须是**同步**的，否则会先按
 * macOS 渲染出一条 38px 留白、拿到结果再跳掉，Windows 上能看见跳动。
 * WKWebView 的 UA 里有 Macintosh，WebView2 里有 Windows。
 */
export const IS_MAC = navigator.userAgent.includes("Macintosh");

document.documentElement.dataset.platform = IS_MAC ? "macos" : "other";
