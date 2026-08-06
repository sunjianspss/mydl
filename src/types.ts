/** 对应 src-tauri/src/engine.rs 的 TorrentView，字段名由 serde 转成 camelCase。 */
export type TorrentState = "initializing" | "live" | "paused" | "error";

export interface TorrentView {
  id: number;
  name: string;
  infoHash: string;
  state: TorrentState;
  error: string | null;
  finished: boolean;
  progressBytes: number;
  totalBytes: number;
  uploadedBytes: number;
  downloadSpeedBps: number;
  uploadSpeedBps: number;
  peersLive: number;
  eta: string | null;
}

/** 对应 engine.rs 的 SessionStatus。底部状态栏用。 */
export interface SessionStatus {
  /** DHT 路由表里的节点数；null 表示 DHT 没启用或还没起来。 */
  dhtNodes: number | null;
  listenPort: number | null;
  /** BT 实际绑定的网卡；null = 跟随系统默认路由。 */
  bindDevice: string | null;
  /**
   * 绑定的网卡现在还在不在。
   *
   * 绑定是建会话时定死的，网卡没了不会自动切换 —— 换网络环境后 BT 会静默
   * 停摆到重启为止。false 时状态栏要报警。没绑定时恒为 true。
   */
  bindDeviceUp: boolean;
}

/** 对应 engine.rs 的 PreviewFile。 */
export interface PreviewFile {
  index: number;
  name: string;
  len: number;
  playable: boolean;
}

/** 对应 engine.rs 的 TorrentPreview。 */
export interface TorrentPreview {
  token: string;
  name: string;
  infoHash: string;
  totalBytes: number;
  files: PreviewFile[];
  alreadyAdded: boolean;
}

/** 对应 search.rs 的 SearchResult。链接全部来自索引器，不是模型生成的。 */
export interface SearchResult {
  title: string;
  magnet: string | null;
  link: string | null;
  size: number;
  seeders: number | null;
  leechers: number | null;
  indexer: string | null;
  /** AI 排序时给的挑选理由；没开 AI 就是 null。 */
  reason: string | null;
  /** true = 模型从网上找来的，不是索引器给的，可能无效。 */
  unverified: boolean;
}

/** 对应 settings.rs 的 Settings。 */
export interface Settings {
  /** null 表示跟随系统默认下载文件夹。 */
  downloadDir: string | null;
  /** 下载完成时发系统通知。 */
  notifyOnComplete: boolean;
  /** 下载完成时播一声提示音。和通知分开，勿扰模式下通知不响但这个会响。 */
  soundOnComplete: boolean;
  /** 完成后移动到这个目录；null 表示不移动。移动后会停止做种。 */
  moveTo: string | null;
  /** 完成后解压内容里的 .zip。 */
  extractArchives: boolean;
  /** 同时最多几个任务在下载。null = 不限。 */
  maxActiveDownloads: number | null;
  /** 分享率到这个值就停止做种。null = 不限。注意每次重启会归零。 */
  seedRatioLimit: number | null;
  /** 所有任务完成后让电脑睡眠。 */
  sleepWhenAllDone: boolean;
  rssFeeds: RssFeed[];
  rssIntervalMinutes: number;
  /** 给所有任务补充公共 tracker。改了要重启 App 才生效。 */
  usePublicTrackers: boolean;
  /** 定期 scrape 公共 tracker，记录做种/下载人数走势。关掉就一个包都不发。 */
  swarmHealthCheck: boolean;
  /** 多久采一次健康度，下限 10 分钟。 */
  swarmHealthIntervalMinutes: number;
  /**
   * BT 流量绑到哪张网卡。null = 跟随系统默认路由。
   *
   * 开着全局 VPN（TUN 模式）时默认路由指向 utun*，BT 也跟着走隧道 ——
   * 隧道出口多半是机房 IP 会被 BT 客户端屏蔽，UPnP 也映射不上导致没有入站。
   * 绑到物理网卡能绕过默认路由直出，同时不影响 VPN 本身。改了要重启。
   */
  bindDevice: string | null;
  /**
   * BT 绑哪张网卡。null = 自动挑一张物理网卡（默认）；
   * "<system>" = 明确跟随系统路由（有 VPN 时 BT 也走 VPN）；
   * 其他 = 具体接口名。见 settings.rs 的 FOLLOW_SYSTEM_ROUTE。
   */
  /** 有任务在下载时阻止电脑休眠。 */
  preventSleepWhileDownloading: boolean;
  /** 切回窗口时看一眼剪贴板里有没有磁力链。只读一次，不后台轮询。 */
  watchClipboard: boolean;
  /** Prowlarr / Jackett 的 Torznab 地址（含 apikey）。 */
  searchUrl: string | null;
  aiBaseUrl: string;
  aiModel: string;
  /** 用模型给搜索结果排序。没填 key 时不起作用。 */
  aiRank: boolean;
  /** 全局上传限速，KiB/s。null 或 0 表示不限。改完立刻生效。 */
  uploadLimitKbps: number | null;
  /** 全局下载限速，KiB/s。改完立刻生效。 */
  downloadLimitKbps: number | null;
  /** socks5://[用户名:密码@]主机:端口。只代理出站 TCP。改了要重启。 */
  proxyUrl: string | null;
  /** IP 黑名单地址。改了要重启。 */
  blocklistUrl: string | null;
  /** 每个任务的 peer 上限。改了要重启。 */
  peerLimit: number | null;
}

/** 对应 settings.rs 的 RssFeed。 */
export interface RssFeed {
  id: string;
  name: string;
  url: string;
  enabled: boolean;
  /** 空格分隔，全部命中才算匹配。 */
  include: string;
  /** 空格分隔，命中任一即排除。 */
  exclude: string;
}

/** 对应 rss.rs 的 CheckReport。 */
export interface CheckReport {
  feed: string;
  total: number;
  matched: number;
  added: number;
  errors: string[];
}

/** 对应 health.rs 的 Sample。一轮采样对一个种子的合并结果。 */
export interface HealthSample {
  /** Unix 秒。 */
  ts: number;
  seeders: number;
  leechers: number;
  /** 这轮有几个 tracker 应答了。0 表示样本不可信，后端判定时会跳过。 */
  trackersOk: number;
}

/** 对应 health.rs 的 Status / Trend。 */
export type HealthStatus = "unknown" | "dead" | "starving" | "ok";
export type HealthTrend = "unknown" | "rising" | "falling" | "flat";

/** 对应 health.rs 的 Verdict。 */
export interface HealthVerdict {
  status: HealthStatus;
  trend: HealthTrend;
  /** 最近一个可信样本；一个都没有就是 null。 */
  latest: HealthSample | null;
  /** 可信样本个数。 */
  samples: number;
  /** 一句话结论，措辞在 Rust 那边定死。 */
  summary: string;
  /** 做种人数曲线，按时间先后。画迷你走势图用。 */
  seedersSeries: number[];
}

/** 对应 diagnose.rs 的 Outcome / Step / Report。 */
export type DiagOutcome = "ok" | "warn" | "bad" | "skipped";

export interface DiagStep {
  /** 查的是什么。 */
  name: string;
  outcome: DiagOutcome;
  /** 实测到的数字，不是解释。 */
  detail: string;
}

export interface DiagReport {
  steps: DiagStep[];
  /** 一句话结论。 */
  verdict: string;
  /** 该怎么办；没有明确建议时为 null。 */
  advice: string | null;
}

/** 对应 verify.rs 的 Level / Finding / VerifyReport。 */
export type VerifyLevel = "ok" | "warn" | "bad";

export interface VerifyFinding {
  level: VerifyLevel;
  text: string;
}

export interface VerifyReport {
  /** 实测到的容器类型；认不出时是「认不出」。 */
  container: string;
  width: number | null;
  height: number | null;
  durationSecs: number | null;
  audioLangs: string[];
  bitrateMbps: number | null;
  findings: VerifyFinding[];
  verdict: string;
}

/** 对应 stats.rs 的 Cell / Report。 */
export interface StatsCell {
  date: string;
  down: number;
  up: number;
  /**
   * 这一天在不在统计范围内。
   *
   * 开始统计之前的日子是「没有数据」，不是「那天没下载」——
   * 图上必须能区分，否则新装的用户会看到半年的「零活动」。
   */
  tracked: boolean;
}

export interface StatsReport {
  /** 从哪天开始统计的。累计值只能从这天算起，说成「历史总量」是撒谎。 */
  since: string | null;
  totalDown: number;
  totalUp: number;
  peakDown: number;
  peakDate: string | null;
  currentStreak: number;
  longestStreak: number;
  activeDays: number;
  /** 按日期升序，最后一格是今天。 */
  cells: StatsCell[];
}

/** 对应 netif.rs 的 NetIf。 */
export interface NetIf {
  /** 接口名，写进设置的就是这个（en0）。 */
  name: string;
  ipv4: string | null;
  /** 看着像隧道接口。绑到隧道上等于没绕过去，界面要标出来。 */
  isTunnel: boolean;
}

/**
 * 对应 forecast.rs 的 Forecast。
 *
 * 注意这里**没有「完成概率」** —— 实测做种数的变异系数 34%~151%，用那种
 * 数据报百分比是编造精度。这里只报测量值。
 */
export interface Forecast {
  /** 实测长期平均速度（字节/秒），不是瞬时速度。 */
  observedBps: number | null;
  /** 算这个速度用了多长窗口（小时）。必须显示 —— 关系到可信度。 */
  windowHours: number | null;
  etaSecs: number | null;
  /** 进度多久没动了（秒）。 */
  stalledSecs: number | null;
  seedersMedian: number | null;
  seedersMin: number | null;
  seedersMax: number | null;
  summary: string;
}

/** 对应 release.rs 的 Candidate。 */
export interface Candidate {
  title: string;
  magnet: string | null;
  link: string | null;
  size: number;
  seeders: number | null;
  leechers: number | null;
  indexer: string | null;
  /** 0~1，和当前任务标题的贴合度。 */
  relevance: number;
  /** 这条就是你现在正在下的那个。 */
  isCurrent: boolean;
  /**
   * 向公共 tracker 实查到的做种数。null = 没查到。
   *
   * 优先显示这个：实测某些中文索引器给所有条目都填 seeders=1、size=0.01GB
   * 这种占位值，照着 `seeders` 选源等于抛硬币。
   */
  liveSeeders: number | null;
  liveLeechers: number | null;
}

/** 对应 lib.rs 的 FoundSources。 */
export interface FoundSources {
  /** 实际用的搜索词。可以在界面上改了重搜。 */
  query: string;
  /** 从任务名解析出的完整标题，打分用的那个。 */
  fullTitle: string;
  candidates: Candidate[];
}

/** 对应 engine.rs 的 FileView。 */
export interface FileView {
  index: number;
  name: string;
  len: number;
  downloaded: number;
  playable: boolean;
  /** 是否在下载范围内。未选中的文件不会被请求。 */
  selected: boolean;
}
