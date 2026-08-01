import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { SearchResult } from "./types";
import { formatBytes } from "./format";
import { clearHistory, forget, history, remember } from "./history";

interface Props {
  /** 选中一条后交给外面走正常的预览-确认流程。 */
  onPick: (uri: string) => void;
  onClose: () => void;
  /** 没配索引器时给一句能照做的提示。 */
  configured: boolean;
  aiEnabled: boolean;
}

export default function SearchDialog({ onPick, onClose, configured, aiEnabled }: Props) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recent, setRecent] = useState<string[]>(history);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    input.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  /// `q` 显式传入而不是读 state：点历史时要「设置输入框 + 立刻搜」，
  /// 而 setQuery 是异步的，这一轮读到的还是旧值。
  async function run(q: string = query) {
    const text = q.trim();
    if (!text || busy) return;

    setQuery(text);
    setBusy(true);
    setError(null);
    // 搜之前就记下来 —— 搜失败的词往往正是要改一改再试的那个。
    setRecent(remember(text));
    try {
      setResults(await invoke<SearchResult[]>("search_torrents", { query: text }));
    } catch (e) {
      setError(String(e));
      setResults(null);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="overlay" onClick={() => !busy && onClose()}>
      <div className="dialog search-dialog" onClick={(e) => e.stopPropagation()}>
        <h2 className="dialog-title">搜索种子</h2>

        <form
          className="search-row"
          onSubmit={(e) => {
            e.preventDefault();
            run();
          }}
        >
          <input
            ref={input}
            className="tb-input"
            value={query}
            placeholder="片名、剧集、关键词…"
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button type="submit" className="tb-btn primary" disabled={busy || !query.trim()}>
            {busy ? "搜索中…" : "搜索"}
          </button>
        </form>

        {recent.length > 0 && (
          <div className="search-history">
            <span className="search-history-label">最近</span>
            {recent.map((q) => (
              <span key={q} className="chip">
                <button
                  className="chip-text"
                  disabled={busy}
                  title={`再搜一次「${q}」`}
                  onClick={() => run(q)}
                >
                  {q}
                </button>
                <button
                  className="chip-x"
                  title="从历史中删除"
                  onClick={() => setRecent(forget(q))}
                >
                  ×
                </button>
              </span>
            ))}
            <button className="act-text" onClick={() => setRecent(clearHistory())}>
              清空
            </button>
          </div>
        )}

        {!configured && (
          <p className="dialog-warn">
            没配索引器，会退而求其次让模型上网找 —— 结果标着「未验证」，
            链接可能是它从网页上抄的，也可能是编的。想要可靠结果，
            在设置里填上自己搭的 Prowlarr / Jackett 的 Torznab 地址。
          </p>
        )}

        {busy && (
          <p className="dialog-sub">
            {configured
              ? `正在查索引器${aiEnabled ? "，拿到结果后还要让模型排一次序" : ""}…`
              : "正在让模型联网找…这一步要先检索再读网页，可能要等一两分钟。"}
          </p>
        )}

        {error && <div className="dialog-warn">{error}</div>}

        {results && !busy && (
          <>
            <div className="dialog-toolbar">
              <span>
                {results.length} 条结果
                {aiEnabled && results.some((r) => r.reason) && " · 已按你的意图重排"}
              </span>
            </div>

            <ul className="dialog-files search-results">
              {results.length === 0 && (
                <li className="files-hint">
                  没搜到匹配的结果。已配置的索引器里可能没有这个内容 ——
                  比如用中文片名搜只收录英文资源的站。换英文名，或者在 Jackett
                  里加几个中文站再试。
                </li>
              )}
              {results.map((r, i) => (
                <li key={i} className="search-item">
                  <div className="search-main">
                    <span className="file-name" title={r.title}>
                      {r.title}
                    </span>
                    <span className="search-meta">
                      <span>{formatBytes(r.size)}</span>
                      {r.seeders != null && <span>种子 {r.seeders}</span>}
                      {r.leechers != null && <span>下载者 {r.leechers}</span>}
                      {r.indexer && <span>{r.indexer}</span>}
                      {!r.magnet && <span className="search-warn">仅 .torrent</span>}
                      {r.unverified && (
                        <span className="search-warn" title="模型从网上找来的，可能无效">
                          未验证
                        </span>
                      )}
                    </span>
                    {r.reason && <span className="search-reason">{r.reason}</span>}
                  </div>
                  <button
                    className="tb-btn"
                    onClick={() => {
                      const uri = r.magnet ?? r.link;
                      if (uri) onPick(uri);
                    }}
                  >
                    添加
                  </button>
                </li>
              ))}
            </ul>

            <p className="files-hint">
              选中后仍然会先真实探测一次 DHT / tracker 才加入任务，
              探不到的链接会明确报错 —— 无论它是索引器给的还是模型排的。
            </p>
          </>
        )}

        <div className="dialog-actions">
          <button onClick={onClose} disabled={busy}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
