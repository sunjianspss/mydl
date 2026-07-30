# mydl

一个 macOS 桌面 BT 下载器。Tauri 2 外壳 + React 界面，协议栈用
[librqbit](https://github.com/ikatson/rqbit)（纯 Rust 实现）。

## 为什么是 librqbit

BitTorrent 协议本身不难，难的是 DHT 健壮性、uTP、NAT 穿透和几百个并发连接的
工程细节。librqbit 已经把这些做完了：磁力链、DHT、HTTP/UDP tracker、IPv6 双栈、
UPnP 端口映射、SOCKS5 代理、会话持久化、边下边播。自研协议栈对自用产品价值为零，
所以这个项目只写产品层。

Rust 生态里不要用 libtorrent 的 binding —— 要过 C++ FFI，交叉编译和维护都是坑。

## 开发

```bash
pnpm install
pnpm tauri dev          # 开发模式，热重载
pnpm tauri build        # 打包出 .app / .dmg
```

冒烟测试会真的连网下载约 1MB 的 Ubuntu 官方镜像，默认跳过：

```bash
cd src-tauri
cargo test --test engine_smoke -- --ignored --nocapture
```

## 结构

```
src/                     React 界面
├── App.tsx              主界面，1 秒轮询一次任务列表
├── types.ts             与 Rust TorrentView 对应的类型
└── format.ts            字节/速度格式化

src-tauri/src/
├── engine.rs            librqbit 会话的封装，不含 Tauri 类型
└── lib.rs               Tauri 命令 + 应用入口
```

`engine.rs` 刻意不依赖 Tauri，以后要加 CLI 或换界面时可以直接复用。

## 已知取舍

- **任务目录记录不持久化。** librqbit 把每个任务的输出目录存在 `pub(crate)`
  字段里读不到，所以 Engine 自己在内存里记了一份。重启后恢复的任务查不到记录，
  「在访达中显示」会退回默认下载目录。
- **界面用 1 秒轮询**而不是事件推送。任务量大时可以改成 Rust 侧 emit 事件。
- **`Speed.mbps` 实际单位是 MiB/s**，不是兆比特。`engine.rs` 里统一换算成字节/秒了。

## 路线图

- [x] 添加磁力链 / 种子文件、暂停继续、删除、进度显示、重启续传
- [ ] 边下边播（librqbit 原生支持分片优先级 + HTTP Range，可直接丢给 IINA/VLC）
- [ ] 完成后自动化（解压、移动、重命名、系统通知）
- [ ] RSS 订阅按规则自动加种

## 合规

客户端本身合法。不要内置指向盗版资源的索引或搜索。
