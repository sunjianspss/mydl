import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Candidate, FoundSources } from "./types";
import { formatBytes } from "./format";

interface Props {
  torrentId: number;
  /** 当前任务已经下了多少字节 —— 换源就是把这些全扔掉，必须写在按钮上。 */
  progressBytes: number;
  onPick: (uri: string) => void;
  onError: (message: string) => void;
}

/**
 * 给当前任务找替代源。
 *
 * 只找、只列，**不自动换**。换源意味着已下的字节全部作废，这是个有代价的
 * 决定，必须由人来做，而且代价要写在按钮上。
 *
 * 搜索词是从任务名解析出来的（`release.rs`），中文压制命名太乱，不可能全对，
 * 所以它是**可编辑的** —— 解析错了用户一眼能看出来并改掉，比默默返回一堆
 * 坏结果强。
 */
export default function BetterSources({ torrentId, progressBytes, onPick, onError }: Props) {
  const [found, setFound] = useState<FoundSources | null>(null);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);

  async function run(q?: string) {
    setBusy(true);
    try {
      const r = await invoke<FoundSources>("find_sources", {
        id: torrentId,
        query: q ?? null,
      });
      setFound(r);
      setQuery(r.query);
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  if (!found) {
    return (
      <div className="sources">
        <button className="act-text" onClick={() => run()} disabled={busy}>
          {busy ? "搜索中…" : "找更好的源"}
        </button>
        <span className="sources-hint">只列出候选，不会自动替换</span>
      </div>
    );
  }

  return (
    <div className="sources sources-open">
      <form
        className="sources-query"
        onSubmit={(e) => {
          e.preventDefault();
          if (query.trim()) run(query);
        }}
      >
        <span className="sources-label">搜索词</span>
        <input
          className="sources-input"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          spellCheck={false}
          title="从任务名自动解析的，解析错了就改这里重搜"
        />
        <button className="act-text" type="submit" disabled={busy || !query.trim()}>
          {busy ? "搜索中…" : "重搜"}
        </button>
        <button className="act-text" type="button" onClick={() => setFound(null)}>
          收起
        </button>
      </form>

      {found.candidates.length === 0 ? (
        <p className="sources-empty">
          没搜到候选。可能是这个词不对 —— 改上面的搜索词再试。
        </p>
      ) : (
        <ul className="sources-list">
          {found.candidates.map((c, i) => (
            <Row
              key={`${c.magnet ?? c.link ?? c.title}-${i}`}
              candidate={c}
              progressBytes={progressBytes}
              onPick={onPick}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

function Row({
  candidate: c,
  progressBytes,
  onPick,
}: {
  candidate: Candidate;
  progressBytes: number;
  onPick: (uri: string) => void;
}) {
  const uri = c.magnet ?? c.link;
  // 相关度是给人判断用的，不是精确分数，所以只分三档，不显示小数。
  const tier = c.relevance >= 0.8 ? "high" : c.relevance >= 0.4 ? "mid" : "low";

  return (
    <li className={`source source-${tier}${c.isCurrent ? " is-current" : ""}`}>
      <div className="source-main">
        <span className="source-title" title={c.title}>
          {c.title}
        </span>
        {c.isCurrent && <span className="source-badge">你正在下这个</span>}
      </div>
      <div className="source-meta">
        {/* 实查到的做种数才有意义。索引器那个字段某些站全填 1，是占位值。 */}
        {c.liveSeeders != null ? (
          <span className="source-seeders" title="向公共 tracker 实查到的">
            {c.liveSeeders} 做种
            {c.liveLeechers != null && c.liveLeechers > 0 && ` · ${c.liveLeechers} 在下`}
          </span>
        ) : (
          <span className="source-seeders source-unknown" title="tracker 没应答，做种数未知">
            做种数未知
          </span>
        )}
        {/* 体积同理，某些站给的是假的，太小就别显示了免得误导 */}
        {c.size > 64 * 1024 * 1024 && <span>{formatBytes(c.size)}</span>}
        {c.indexer && <span className="source-indexer">{c.indexer}</span>}
        <span className="spacer" />
        {c.isCurrent ? (
          <span className="sources-hint">当前源</span>
        ) : (
          <button
            className="act-text"
            disabled={!uri}
            title={
              progressBytes > 0
                ? `会新建一个任务，从 0 开始下。当前任务和它已下的 ${formatBytes(
                    progressBytes,
                  )} 都不会被动 —— 要不要删由你决定`
                : "会新建一个任务"
            }
            onClick={() => uri && onPick(uri)}
          >
            {/* 不写「丢弃」：这里不删任何东西，只是新任务从 0 开始，
                已下的那些在新压制版里用不上。措辞必须和实际行为对得上。 */}
            {progressBytes > 0 ? `从头下（现有 ${formatBytes(progressBytes)} 用不上）` : "添加"}
          </button>
        )}
      </div>
    </li>
  );
}
