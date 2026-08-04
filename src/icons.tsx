/**
 * 界面里用到的图标。全部内联 SVG —— 打包版没有网络，外链图标字体不可靠；
 * 而且系统 SF Symbols 在 webview 里拿不到，只能自己画。
 *
 * 统一 16×16 视口、1.5 描边，这样放大缩小时线宽看起来一致。
 */

interface Props {
  className?: string;
}

const stroke = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
};

function Svg({ children, className }: Props & { children: React.ReactNode }) {
  return (
    <svg viewBox="0 0 16 16" className={className} aria-hidden="true">
      {children}
    </svg>
  );
}

/* ---------- 文件类型（行首那块图标底板里用） ---------- */

export function VideoIcon(p: Props) {
  return (
    <Svg {...p}>
      <rect x="1.8" y="3.5" width="12.4" height="9" rx="1.6" {...stroke} />
      <path d="M5.2 3.5v9M10.8 3.5v9M1.8 8h12.4" {...stroke} />
    </Svg>
  );
}

export function AudioIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M6 12V4.2l6-1.2V11" {...stroke} />
      <circle cx="4.4" cy="12" r="1.7" {...stroke} />
      <circle cx="10.4" cy="10.8" r="1.7" {...stroke} />
    </Svg>
  );
}

export function DiscIcon(p: Props) {
  return (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" {...stroke} />
      <circle cx="8" cy="8" r="1.6" {...stroke} />
    </Svg>
  );
}

export function ArchiveIcon(p: Props) {
  return (
    <Svg {...p}>
      <rect x="2.5" y="3" width="11" height="10" rx="1.6" {...stroke} />
      <path d="M2.5 6h11M7 3v3M9 6v2.5M7 8.5h2" {...stroke} />
    </Svg>
  );
}

export function FileIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M4 2.5h4.5L12 6v7.5H4z" {...stroke} />
      <path d="M8.5 2.5V6H12" {...stroke} />
    </Svg>
  );
}

export function AlertIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M7 2.5 2.5 12a1 1 0 0 0 .9 1.5h9.2a1 1 0 0 0 .9-1.5L9 2.5a1.1 1.1 0 0 0-2 0z" {...stroke} />
      <path d="M8 6.2v3.1M8 11.3v.1" {...stroke} />
    </Svg>
  );
}

/* ---------- 侧边栏 ---------- */

export function StackIcon(p: Props) {
  return (
    <Svg {...p}>
      <rect x="2" y="3" width="12" height="10" rx="2" {...stroke} />
      <path d="M2 6.5h12" {...stroke} />
    </Svg>
  );
}

export function DownIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M8 2.5v8M4.5 7.5 8 11l3.5-3.5M2.5 13h11" {...stroke} />
    </Svg>
  );
}

export function UpIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M8 13.5v-8M4.5 8.5 8 5l3.5 3.5M2.5 3h11" {...stroke} />
    </Svg>
  );
}

export function CheckIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M3 8.5 6.5 12 13 5" {...stroke} />
    </Svg>
  );
}

export function RssIcon(p: Props) {
  return (
    <Svg {...p}>
      <circle cx="4" cy="12" r="1.3" fill="currentColor" />
      <path d="M3 8.2a4.8 4.8 0 0 1 4.8 4.8" {...stroke} />
      <path d="M3 4.2A8.8 8.8 0 0 1 11.8 13" {...stroke} />
    </Svg>
  );
}

export function FolderIcon(p: Props) {
  return (
    <Svg {...p}>
      <path
        d="M2 5a1.5 1.5 0 0 1 1.5-1.5h2.2l1.3 1.6h5.5A1.5 1.5 0 0 1 14 6.6v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5z"
        {...stroke}
      />
    </Svg>
  );
}

/* ---------- 操作 ---------- */

export function PlusIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M8 3.5v9M3.5 8h9" {...stroke} strokeWidth={1.8} />
    </Svg>
  );
}

/**
 * 设置用滑杆而不是齿轮：16px 视口画不出齿，圆圈加八根辐条看起来是个太阳。
 */
export function GearIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" {...stroke} />
      <circle cx="6" cy="4.5" r="1.5" {...stroke} />
      <circle cx="10.5" cy="8" r="1.5" {...stroke} />
      <circle cx="5.5" cy="11.5" r="1.5" {...stroke} />
    </Svg>
  );
}

export function SearchIcon(p: Props) {
  return (
    <Svg {...p}>
      <circle cx="7" cy="7" r="4.5" {...stroke} />
      <path d="M10.4 10.4 14 14" {...stroke} />
    </Svg>
  );
}

export function SunIcon(p: Props) {
  return (
    <Svg {...p}>
      <circle cx="8" cy="8" r="3" {...stroke} />
      <path
        d="M8 1.4v1.7M8 12.9v1.7M14.6 8h-1.7M3.1 8H1.4M12.7 3.3l-1.2 1.2M4.5 11.5l-1.2 1.2M12.7 12.7l-1.2-1.2M4.5 4.5 3.3 3.3"
        {...stroke}
      />
    </Svg>
  );
}

export function MoonIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M13.2 9.6A5.6 5.6 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z" {...stroke} />
    </Svg>
  );
}

export function PauseIcon(p: Props) {
  return (
    <Svg {...p}>
      <rect x="4.6" y="3.6" width="2.5" height="8.8" rx="1.1" fill="currentColor" />
      <rect x="8.9" y="3.6" width="2.5" height="8.8" rx="1.1" fill="currentColor" />
    </Svg>
  );
}

export function PlayIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M5 3.6v8.8l7-4.4z" fill="currentColor" />
    </Svg>
  );
}

/// 全部暂停：两条竖杠外面套个圈，和单任务的暂停区分开。
export function PauseAllIcon(p: Props) {
  return (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" {...stroke} />
      <path d="M6.4 5.8v4.4M9.6 5.8v4.4" {...stroke} />
    </Svg>
  );
}

/** 上传箭头 + 暂停条：只停做种。和「全部暂停」的圆圈款刻意长得不一样。 */
export function PauseSeedIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M4.6 12.4v-8M2 7 4.6 4.4 7.2 7" {...stroke} />
      <rect x="9.6" y="4.4" width="1.9" height="8" rx="0.95" fill="currentColor" />
      <rect x="12.6" y="4.4" width="1.9" height="8" rx="0.95" fill="currentColor" />
    </Svg>
  );
}

export function PlayAllIcon(p: Props) {
  return (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" {...stroke} />
      <path d="M6.6 5.5v5l4-2.5z" fill="currentColor" stroke="none" />
    </Svg>
  );
}

export function TrashIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M3.5 4.5h9M6.5 4.5V3h3v1.5M5 4.5l.6 8h4.8l.6-8" {...stroke} />
    </Svg>
  );
}

export function DocIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M4 2.5h4.5L12 6v7.5H4z" {...stroke} />
      <path d="M8.5 2.5V6H12M6 9h4M6 11h3" {...stroke} />
    </Svg>
  );
}

export function LinkIcon(p: Props) {
  return (
    <Svg {...p}>
      <path d="M6.6 9.4a2.6 2.6 0 0 0 3.7 0l2-2a2.6 2.6 0 0 0-3.7-3.7l-.9.9" {...stroke} />
      <path d="M9.4 6.6a2.6 2.6 0 0 0-3.7 0l-2 2a2.6 2.6 0 0 0 3.7 3.7l.9-.9" {...stroke} />
    </Svg>
  );
}

/* ---------- 按扩展名挑图标 ---------- */

const VIDEO = ["mp4", "m4v", "mkv", "webm", "avi", "mov", "ts", "m2ts", "flv", "wmv", "mpg", "mpeg"];
const AUDIO = ["mp3", "m4a", "aac", "flac", "wav", "ogg", "opus", "wma"];
const DISC = ["iso", "img", "dmg", "bin", "cue"];
const ARCHIVE = ["zip", "rar", "7z", "tar", "gz", "xz", "bz2"];

export type IconKind = "video" | "audio" | "disc" | "archive" | "file" | "dead";

/**
 * 从任务名猜文件类型。多文件种子的名字通常不带扩展名（是个目录名），
 * 猜不出来就退回通用文件图标 —— 猜错比没有更糟。
 */
export function iconKindFor(name: string, errored: boolean): IconKind {
  if (errored) return "dead";

  const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  if (VIDEO.includes(ext)) return "video";
  if (AUDIO.includes(ext)) return "audio";
  if (DISC.includes(ext)) return "disc";
  if (ARCHIVE.includes(ext)) return "archive";

  // 影视资源的目录名里几乎必有这些标记，比扩展名更可靠。
  if (/\b(1080p|2160p|720p|4k|web-?dl|bluray|blu-ray|hdtv|x264|x265|h264|h265|hevc|remux)\b/i.test(name))
    return "video";

  return "file";
}

export function TypeIcon({ kind }: { kind: IconKind }) {
  switch (kind) {
    case "video":
      return <VideoIcon />;
    case "audio":
      return <AudioIcon />;
    case "disc":
      return <DiscIcon />;
    case "archive":
      return <ArchiveIcon />;
    case "dead":
      return <AlertIcon />;
    default:
      return <FileIcon />;
  }
}
