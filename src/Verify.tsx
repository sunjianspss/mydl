import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { VerifyLevel, VerifyReport } from "./types";

interface Props {
  torrentId: number;
  onError: (message: string) => void;
}

const MARK: Record<VerifyLevel, string> = { ok: "✓", warn: "!", bad: "✕" };

/**
 * 验伪：这个文件是不是名字说的那个东西。
 *
 * 只读文件头（必要时加文件尾）几 MB，从容器元数据里读真实分辨率、时长和
 * 音轨 —— 一帧都不解码，见 `media.rs`。所以 14 GB 的种子也能在下完之前验。
 *
 * 会把这几个分片提到最高优先级，对正在下的任务等于插了个队，所以只在点了
 * 才跑。
 */
export default function Verify({ torrentId, onError }: Props) {
  const [report, setReport] = useState<VerifyReport | null>(null);
  const [busy, setBusy] = useState(false);

  async function run() {
    setBusy(true);
    setReport(null);
    try {
      setReport(await invoke<VerifyReport>("verify_torrent", { id: torrentId }));
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const mins = report?.durationSecs != null ? Math.round(report.durationSecs / 60) : null;

  return (
    <div className="diag">
      <div className="diag-bar">
        <button className="act-text" onClick={run} disabled={busy}>
          {busy ? "读取文件头…" : "验一下是不是货真价实"}
        </button>
        <span className="sources-hint">
          {busy ? "只取头部几 MB，不用等下完" : "从容器里读真实分辨率和音轨，跟名字对一下"}
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
          <div className="verify-spec">
            <span>{report.container}</span>
            {report.width != null && (
              <span>
                {report.width}×{report.height}
              </span>
            )}
            {mins != null && <span>{mins} 分钟</span>}
            {report.bitrateMbps != null && <span>{report.bitrateMbps.toFixed(1)} Mbps</span>}
            {report.audioLangs.length > 0 && <span>音轨 {report.audioLangs.join(" / ")}</span>}
          </div>

          <ul className="diag-steps">
            {report.findings.map((f, i) => (
              <li key={i} className={`diag-step diag-${f.level}`}>
                <span className="diag-mark">{MARK[f.level]}</span>
                <span className="verify-text">{f.text}</span>
              </li>
            ))}
          </ul>
          <p className="diag-verdict">{report.verdict}</p>
        </>
      )}
    </div>
  );
}
