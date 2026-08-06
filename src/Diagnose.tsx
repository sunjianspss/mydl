import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { DiagOutcome, DiagReport } from "./types";

interface Props {
  torrentId: number;
  onError: (message: string) => void;
}

const MARK: Record<DiagOutcome, string> = {
  ok: "✓",
  warn: "!",
  bad: "✕",
  skipped: "–",
};

/**
 * 「为什么不动？」
 *
 * 任务卡住时，六种完全不同的原因表现一模一样：进度不动、0 peers。这个按钮
 * 把手工要跑两天的取证阶梯压成几十秒，见 `diagnose.rs`。
 *
 * 会真的发包（tracker announce + 试握手 + 打一个对照 swarm），所以只在点了
 * 才跑，不做后台巡检 —— 后台偷偷连一堆 peer 是很不礼貌的。
 */
export default function Diagnose({ torrentId, onError }: Props) {
  const [report, setReport] = useState<DiagReport | null>(null);
  const [busy, setBusy] = useState(false);

  async function run() {
    setBusy(true);
    setReport(null);
    try {
      setReport(await invoke<DiagReport>("diagnose_torrent", { id: torrentId }));
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="diag">
      <div className="diag-bar">
        <button className="act-text" onClick={run} disabled={busy}>
          {busy ? "诊断中…" : "为什么不动？"}
        </button>
        <span className="sources-hint">
          {busy ? "在连 tracker 和 peer，可能要一分多钟" : "查一遍网络、swarm 和握手，只读不改"}
        </span>
        {report && (
          <>
            <span className="spacer" />
            <button className="act-text" onClick={() => setReport(null)}>
              收起
            </button>
          </>
        )}
      </div>

      {report && (
        <>
          <ul className="diag-steps">
            {report.steps.map((s, i) => (
              <li key={i} className={`diag-step diag-${s.outcome}`}>
                <span className="diag-mark">{MARK[s.outcome]}</span>
                <span className="diag-name">{s.name}</span>
                <span className="diag-detail">{s.detail}</span>
              </li>
            ))}
          </ul>
          <p className="diag-verdict">{report.verdict}</p>
          {report.advice && <p className="diag-advice">{report.advice}</p>}
        </>
      )}
    </div>
  );
}
