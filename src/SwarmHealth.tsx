import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Forecast, HealthVerdict } from "./types";

interface Props {
  torrentId: number;
  infoHash: string;
  onError: (message: string) => void;
}

const STATUS_LABEL: Record<HealthVerdict["status"], string> = {
  unknown: "数据不足",
  dead: "没人做种",
  starving: "源太少",
  ok: "正常",
};

const TREND_ARROW: Record<HealthVerdict["trend"], string> = {
  rising: "↗",
  falling: "↘",
  flat: "→",
  unknown: "",
};

/**
 * swarm 健康度。展开任务时显示。
 *
 * 只读本地已经采好的历史，不发包 —— 所以展开任务不会顺带把 info-hash
 * 捅给一堆 tracker。真要立刻采一轮得点「现在查一次」，那是显式动作。
 */
export default function SwarmHealth({ torrentId, infoHash, onError }: Props) {
  const [verdict, setVerdict] = useState<HealthVerdict | null>(null);
  const [fc, setFc] = useState<Forecast | null>(null);
  const [checking, setChecking] = useState(false);

  const load = useCallback(async () => {
    try {
      setVerdict(await invoke<HealthVerdict>("torrent_health", { infoHash }));
      setFc(await invoke<Forecast>("torrent_forecast", { id: torrentId }));
    } catch (e) {
      onError(String(e));
    }
  }, [infoHash, torrentId, onError]);

  useEffect(() => {
    load();
  }, [load]);

  async function checkNow() {
    setChecking(true);
    try {
      await invoke("check_health_now");
      await load();
    } catch (e) {
      onError(String(e));
    } finally {
      setChecking(false);
    }
  }

  if (!verdict) return null;

  return (
    <div className={`health health-${verdict.status} health-wrap`}>
      <span className="health-badge">{STATUS_LABEL[verdict.status]}</span>
      <span className="health-summary">{verdict.summary}</span>
      {verdict.trend !== "unknown" && (
        <span className="health-trend" title="按前后两半的均值比算的">
          {TREND_ARROW[verdict.trend]}
        </span>
      )}
      <Sparkline values={verdict.seedersSeries} />
      <span className="spacer" />
      {verdict.samples > 0 && (
        <span className="health-samples" title="采到过几轮可信数据">
          {verdict.samples} 次采样
        </span>
      )}
      <button className="act-text" onClick={checkNow} disabled={checking}>
        {checking ? "查询中…" : "现在查一次"}
      </button>
      {/* 「还要多久」单独一行：它是按实测吞吐量算的，和上面那行 swarm 状态
          是两回事，混在一起容易让人以为做种数能推出剩余时间。 */}
      {fc && fc.summary && (
        <div className="health-forecast" title="按采样历史里的实测速度算的，不是瞬时速度">
          {fc.summary}
          {fc.seedersMin != null && fc.seedersMax != null && fc.seedersMin !== fc.seedersMax && (
            <span className="health-range">
              　做种数区间 {fc.seedersMin}~{fc.seedersMax}，中位 {fc.seedersMedian}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * 做种人数的迷你走势图。
 *
 * 少于两个点就不画 —— 一个点连不成线，画出来是条误导人的平线。
 * 纵轴从 0 起而不是从最小值起：做种数的绝对值本身有意义，
 * 3 到 4 和 300 到 400 不该看起来一样陡。
 */
function Sparkline({ values }: { values: number[] }) {
  if (values.length < 2) return null;

  const w = 64;
  const h = 14;
  const max = Math.max(...values, 1);
  const step = w / (values.length - 1);
  const points = values
    .map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * h).toFixed(1)}`)
    .join(" ");

  return (
    <svg className="health-spark" viewBox={`0 0 ${w} ${h}`} aria-hidden="true">
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.2" />
    </svg>
  );
}
