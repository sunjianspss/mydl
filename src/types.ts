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

/** 对应 settings.rs 的 Settings。 */
export interface Settings {
  /** null 表示跟随系统默认下载文件夹。 */
  downloadDir: string | null;
  /** 下载完成时发系统通知。 */
  notifyOnComplete: boolean;
  /** 完成后移动到这个目录；null 表示不移动。移动后会停止做种。 */
  moveTo: string | null;
  /** 完成后解压内容里的 .zip。 */
  extractArchives: boolean;
  rssFeeds: RssFeed[];
  rssIntervalMinutes: number;
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
