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
  rssFeeds: RssFeed[];
  rssIntervalMinutes: number;
  /** 给所有任务补充公共 tracker。改了要重启 App 才生效。 */
  usePublicTrackers: boolean;
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
