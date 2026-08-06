import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { StatsCell, StatsReport } from "./types";
import { formatBytes } from "./format";

interface Props {
  onError: (message: string) => void;
}

type Mode = "daily" | "weekly" | "cumulative";

const MODES: { id: Mode; label: string }[] = [
  { id: "daily", label: "按天" },
  { id: "weekly", label: "按周" },
  { id: "cumulative", label: "累计" },
];

/** 热力图分几档。和 CSS 里的 --heat-1..5 对应。 */
const LEVELS = 5;

/**
 * 下载统计。KPI 行 + 活动热力图。
 *
 * # 几条刻意的取舍
 *
 * **不叫「历史总量」。** 这个系统是从加上这个功能那天才开始记的，
 * 把累计值说成 lifetime 是撒谎。所以标签写「开始统计以来」并显示起始日期。
 *
 * **「没有数据」和「那天没下载」用不同的颜色。** 前者是空格子（中性灰），
 * 后者是色阶最底那一档。混在一起的话，新装的用户会看到半年的「零活动」，
 * 以为自己什么都没下过。
 *
 * **累计换成折线，不用热力图。** 累计值单调递增，画成热力图就是「越往后
 * 越深」，看不出任何东西 —— 那是「随时间变化」的活儿，该用折线。
 */
export default function Stats({ onError }: Props) {
  const [report, setReport] = useState<StatsReport | null>(null);
  const [mode, setMode] = useState<Mode>("daily");
  const [showTable, setShowTable] = useState(false);

  useEffect(() => {
    invoke<StatsReport>("download_stats").then(setReport).catch((e) => onError(String(e)));
  }, [onError]);

  if (!report) return <div className="stats-empty">读取中…</div>;

  const noData = report.since === null;

  return (
    <div className="stats-view">
      <KpiRow report={report} />
      {report.since && (
        <p className="stats-since">
          统计自 {report.since} —— 之前的数据这个系统没记过，累计值只能从这天算起
        </p>
      )}

      <div className="stats-head">
        <h3 className="stats-title">下载活动</h3>
        <div className="stats-modes">
          {MODES.map((m) => (
            <button
              key={m.id}
              className="stats-mode"
              aria-pressed={mode === m.id}
              onClick={() => setMode(m.id)}
            >
              {m.label}
            </button>
          ))}
        </div>
      </div>

      {noData ? (
        <p className="stats-empty">
          还没开始统计。App 每 30 分钟记一次，跑一会儿这里就有数据了。
        </p>
      ) : mode === "cumulative" ? (
        <Cumulative cells={report.cells} />
      ) : (
        <Heatmap cells={report.cells} weekly={mode === "weekly"} />
      )}

      {!noData && (
        <>
          <button className="act-text stats-tablebtn" onClick={() => setShowTable((v) => !v)}>
            {showTable ? "收起数据表" : "看数据表"}
          </button>
          {showTable && <DataTable cells={report.cells} />}
        </>
      )}
    </div>
  );
}

function KpiRow({ report: r }: { report: StatsReport }) {
  const tiles: { label: string; value: string; title?: string }[] = [
    {
      label: "累计下载",
      value: formatBytes(r.totalDown),
      title: "只能从开始统计那天算起 —— 之前的数据这个系统没记过",
    },
    { label: "累计上传", value: formatBytes(r.totalUp) },
    {
      label: "单日最高",
      value: formatBytes(r.peakDown),
      title: r.peakDate ? `出现在 ${r.peakDate}` : undefined,
    },
    { label: "当前连续", value: `${r.currentStreak} 天` },
    { label: "最长连续", value: `${r.longestStreak} 天` },
  ];
  return (
    <div className="kpi-row">
      {tiles.map((t) => (
        <div className="kpi" key={t.label} title={t.title}>
          <div className="kpi-value">{t.value}</div>
          <div className="kpi-label">{t.label}</div>
        </div>
      ))}
    </div>
  );
}

/** 把日格子按周聚合。每周从最早那天起算 7 天一组。 */
function toWeeks(cells: StatsCell[]): StatsCell[] {
  const out: StatsCell[] = [];
  for (let i = 0; i < cells.length; i += 7) {
    const chunk = cells.slice(i, i + 7);
    out.push({
      date: `${chunk[0].date} ~ ${chunk[chunk.length - 1].date}`,
      down: chunk.reduce((s, c) => s + c.down, 0),
      up: chunk.reduce((s, c) => s + c.up, 0),
      tracked: chunk.some((c) => c.tracked),
    });
  }
  return out;
}

/**
 * 分档：把字节数映射到 0~LEVELS。
 *
 * 用**分位数**而不是等分最大值：下载量的分布极偏（一天 50 GB、其余几百 MB），
 * 等分的话除了峰值那天全是最浅一档，图上什么都看不出来。
 */
function levelize(values: number[]): (v: number) => number {
  const positive = values.filter((v) => v > 0).sort((a, b) => a - b);
  if (positive.length === 0) return () => 0;
  const cuts = Array.from(
    { length: LEVELS - 1 },
    (_, i) => positive[Math.floor(((i + 1) / LEVELS) * (positive.length - 1))],
  );
  return (v) => {
    if (v <= 0) return 0;
    let lvl = 1;
    for (const c of cuts) if (v > c) lvl++;
    return Math.min(lvl, LEVELS);
  };
}

function Heatmap({ cells, weekly }: { cells: StatsCell[]; weekly: boolean }) {
  const data = weekly ? toWeeks(cells) : cells;
  const level = useMemo(() => levelize(data.map((c) => c.down)), [data]);

  return (
    <>
      <div className={`heat${weekly ? " heat-weekly" : ""}`} role="img" aria-label="下载活动热力图">
        {data.map((c) => {
          const lvl = c.tracked ? level(c.down) : -1;
          return (
            <div
              key={c.date}
              className={`heat-cell heat-l${lvl}`}
              title={
                !c.tracked
                  ? `${c.date}　没有统计数据`
                  : `${c.date}　↓ ${formatBytes(c.down)}　↑ ${formatBytes(c.up)}`
              }
            />
          );
        })}
      </div>
      <div className="heat-legend">
        <span>没数据</span>
        <span className="heat-cell heat-l-1" />
        <span className="heat-gap">少</span>
        {[0, 1, 2, 3, 4, 5].map((l) => (
          <span key={l} className={`heat-cell heat-l${l}`} />
        ))}
        <span>多</span>
      </div>
    </>
  );
}

/**
 * 累计曲线。单调递增，所以是折线不是热力图。
 *
 * 只画开始统计之后的部分 —— 之前那段是没数据，不是 0。
 */
function Cumulative({ cells }: { cells: StatsCell[] }) {
  const tracked = cells.filter((c) => c.tracked);
  if (tracked.length < 2) {
    return <p className="stats-empty">至少要两天数据才画得出累计曲线。</p>;
  }

  let acc = 0;
  const points = tracked.map((c) => {
    acc += c.down;
    return { date: c.date, total: acc };
  });
  const max = points[points.length - 1].total || 1;
  const w = 100;
  const h = 28;
  const path = points
    .map((p, i) => `${(i / (points.length - 1)) * w},${h - (p.total / max) * h}`)
    .join(" ");

  return (
    <div className="cume">
      <svg viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-label="累计下载量曲线">
        <polyline points={path} className="cume-line" vectorEffect="non-scaling-stroke" />
      </svg>
      <div className="cume-axis">
        <span>{points[0].date}</span>
        <span className="cume-total">共 {formatBytes(max)}</span>
        <span>{points[points.length - 1].date}</span>
      </div>
    </div>
  );
}

/** 无障碍要求：颜色之外必须有一条读得到数字的路。 */
function DataTable({ cells }: { cells: StatsCell[] }) {
  const rows = cells.filter((c) => c.tracked && (c.down > 0 || c.up > 0)).reverse();
  if (rows.length === 0) {
    return <p className="stats-empty">统计范围内还没有任何流量。</p>;
  }
  return (
    <table className="stats-table">
      <thead>
        <tr>
          <th>日期</th>
          <th>下载</th>
          <th>上传</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((c) => (
          <tr key={c.date}>
            <td>{c.date}</td>
            <td>{formatBytes(c.down)}</td>
            <td>{formatBytes(c.up)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
