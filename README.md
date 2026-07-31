# mydl

一个 macOS 桌面 BT 下载器。Tauri 2 外壳 + React 界面，协议栈用
[librqbit](https://github.com/ikatson/rqbit)（纯 Rust 实现）。

## 为什么必须用 librqbit 9（uTP）

**8.x 只支持 TCP**，连接 peer 走 `tokio::net::TcpStream`，依赖里没有任何
uTP 实现。现代客户端默认走 uTP（UDP 上的传输协议，NAT 穿透好得多），
家用宽带后面的做种者基本只能靠 uTP 连上。

后果很具体：Debian/Ubuntu 官方种子由开着 TCP 端口的服务器做种，能连；
而普通资源的做种者连不上，磁力链的元信息就永远拿不到，表现为
「解析磁力链超时」。同一条链接在 qBittorrent 里却能下。

注意它换来的是**连通性**，不是「礼让」—— 这套 uTP 没有 LEDBAT，见「上传限速」。

9.0.0-rc.0 引入 `librqbit-utp`，`ListenerMode::TcpAndUtp` 同时监听两者。
注意上游把默认值仍留在 `TcpOnly`，代码里写着
`// TODO: once uTP is stable upgrade default to both` —— 我们是主动开启的，
如果哪天 uTP 出问题，改回 `TcpOnly` 即可。

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

`pnpm tauri dev` 编出来的二进制前端指向 vite 开发服务器，**不能脱离
`tauri dev` 单独运行**（会白屏）。要独立运行的版本得用 `tauri build`。

产物在 `src-tauri/target/release/bundle/`。安装：

```bash
ditto src-tauri/target/release/bundle/macos/mydl.app /Applications/mydl.app
```

## Windows

平台差异全在 `src-tauri/src/platform.rs`，别的模块里不该再出现 `cfg(target_os)`：

| | macOS | Windows |
|---|---|---|
| 阻止休眠 | `caffeinate -i -m -s -w <pid>` 子进程 | `SetThreadExecutionState` |
| 日志目录 | `~/Library/Logs/mydl` | `%LOCALAPPDATA%\mydl\logs` |
| 播放器 | 扫 `/Applications` 找 `.app` | 扫 Program Files 找 VLC / mpv / PotPlayer / MPC-HC |
| 起播 | `open -a <播放器> <地址>` | 直接 `播放器.exe <地址>` |

`SetThreadExecutionState` **是按线程记的**，所以 `keep_awake::spawn` 用独立
OS 线程而不是 tokio 任务 —— 任务在 worker 之间迁移的话，解除会发生在另一个
线程上，原线程那份要求永远留着，电脑再也不会自己睡。循环体本来也全是同步
调用，不需要 async。

界面上的标题栏留白只在 macOS 出现（`[data-platform="macos"]`）。Windows 的
窗口控件在右上角，留着就是一条空白。判定走 userAgent 而不是调 Rust 命令：
必须**同步**拿到，否则会先渲染出 38px 留白再跳掉。

**在 Windows 上打包**（不支持从 macOS 交叉编译，Tauri 的 NSIS/MSI 需要
Windows 侧工具链）：

```powershell
# 先装 Rust、Node + pnpm，以及 VS Build Tools 的「使用 C++ 的桌面开发」
pnpm install
pnpm tauri build      # 产物在 src-tauri\target\release\bundle\nsis\
```

首次运行时 Windows 防火墙会问是否允许 4240 端口，**要点允许**，否则连不上 peer。
Win10 还需要 WebView2 运行时（Win11 自带）。

## 上传限速

设置里「限制上传速度」，单位 KB/s，默认不限；勾上后初始值 128 KB/s（约 1 Mbps），
可以填到 1。改完**立刻生效**，不用重启 —— librqbit 的限速器
（`Session.ratelimits`）是运行时可换的。

限的是**上传**，而且不只做种：下载中的任务同样在往外传（tit-for-tat），
所以两种情况都受这个值约束。

为什么需要它：**这套 uTP 并不「礼让」**。BEP29 的卖点是 LEDBAT 拥塞控制，
探测到排队延迟上升就主动退让，所以正经 uTP 客户端做种时不会影响别人上网。
但 `librqbit-utp` 0.7 用的是 CUBIC（`src/congestion/` 下只有 `cubic.rs`，
`lib.rs` 开头还留着 `// TODO: LEDBAT congestion control`），抢带宽和普通 TCP
一样凶。上行一满，ACK 跟着排队，同一条线路上刷网页、开会全都卡。

所以这里的 uTP 只兑现了「NAT 穿透」那一半好处，「不打扰别人」那一半得靠限速
自己补。**代价是下载也会变慢** —— BT 靠上传换下载，限太死会被别人降速，
低于 32 KB/s 就比较明显了。

只有全局限速，没有按任务限速：v9 的 per-torrent `ratelimits` 在
`ManagedTorrentOptions` 里，整个结构是 `pub(crate)`，外部够不着。

## 下载期间不休眠

默认开启。`keep_awake.rs` 每 15 秒看一次有没有「正在下载」的任务，有就拉起
一个 `caffeinate -i -m -s` 子进程，没有就杀掉。**做种不算** —— 没道理为了
给别人上传就让电脑整夜不睡。

用 `caffeinate` 而不是直接调 IOKit：少一层 FFI，而且 `pmset -g assertions`
里能看到是谁在阻止休眠。参数里带 `-w <自己的 pid>`，万一 App 被强杀、
来不及 kill 子进程，caffeinate 也会跟着退出，不会留个进程让电脑永远睡不着。

**合盖仍然会睡**，这是系统行为，任何软件都拦不住。想挂整夜别合盖。
接电源时 `-s` 才有效；用电池时靠 `-i`，电量耗尽照样会睡。

## 磁力链下不动时怎么查

三个诊断工具，都是 `#[ignore]`，手动跑：

```bash
cd src-tauri

# 1. 磁力链路径本身通不通（拿 Debian 官方种子做对照）
cargo test --test magnet_diag -- --ignored --nocapture

# 2. DHT 里到底有没有这个 swarm 的 peer
MYDL_PROBE_HASH=<40位十六进制> \
  cargo test --test dht_probe -- --ignored --nocapture

# 3. 连 peer 时具体发生了什么（peer 级 debug 日志）
MYDL_PROBE_MAGNET='magnet:?xt=...' \
  cargo test --test peer_debug -- --ignored --nocapture
```

判读方式：

| 现象 | 说明 |
|---|---|
| 对照组也查不到 peer | DHT 本身有问题（网络、路由表） |
| 对照几百个 peer、待查 0 个 | 这个 swarm 不在 DHT 里：私有种子（`private` 标志会禁用 DHT/PEX，必须用带 passkey 的 .torrent），或者真没人做种 |
| 待查有几个 peer 但仍超时 | swarm 太瘦。`read_metainfo_from_peer_receiver` 用 `seen` 集合保证每个地址只试一次，如果只有两三个 peer 且都不给元信息，就没有别的可试了 —— 这种情况开公共 tracker 开关能多找到一些源 |

## 日志

写在 `~/Library/Logs/mydl/mydl.<日期>.log`，按天轮转、保留 7 天。
界面右上角「日志」按钮直接在访达里打开该目录。

打包后 stdout 没人接，所以文件日志是唯一的排查手段。

`cargo test` 跑不联网的部分：Range 解析单元测试，以及子目录回归测试（本地现造
种子，1 秒跑完）。两个联网冒烟测试会真的下载几 MB，默认被 `#[ignore]` 跳过：

```bash
cd src-tauri
cargo test                                                # 不联网
cargo test --test engine_smoke -- --ignored --nocapture   # 真实下载
cargo test --test stream_smoke -- --ignored --nocapture   # 边下边播 + Range
```

## 结构

```
src/                     React 界面
├── App.tsx              主界面，1 秒轮询一次任务列表
├── FileList.tsx         展开的文件列表 + 播放按钮
├── icons.tsx            内联 SVG 图标 + 按文件名判类型
├── types.ts             与 Rust 侧对应的类型
└── format.ts            字节/速度格式化

src-tauri/src/
├── engine.rs            librqbit 会话的封装，不含 Tauri 类型
├── platform.rs          平台差异（日志目录、播放器、阻止休眠）
├── stream_server.rs     本地 HTTP 流媒体服务（Range 支持）
├── settings.rs          持久化设置（JSON，原子写）
└── lib.rs               Tauri 命令 + 应用入口
```

设置存在 `~/Library/Application Support/com.sun.mydl/settings.json`，
同目录下还有 `rss_seen.json`（已处理过的 RSS 条目）和 `output_folders.json`
（任务的自定义输出目录）。
以后 RSS 规则、自动化动作往 `Settings` 里加字段即可 —— `#[serde(default)]`
保证旧配置文件读得进来。

`engine.rs` 刻意不依赖 Tauri，以后要加 CLI 或换界面时可以直接复用。

## 界面

三段式骨架，照系统自带应用的样子搭：**左侧分类栏 + 主区 + 底部状态栏**。

之前是一个平铺列表扔在空画布上，整屏纯文字。问题不在配色和圆角，在于没有
信息架构 —— 所以这次改的是结构。

**左侧分类栏**：全部 / 下载中 / 做种中 / 已完成 / 已暂停，带实时计数，下面
是 RSS 入口和当前下载目录。计数和过滤共用同一个 `matchesFilter`，不会出现
「写着 2 个、点进去只有 1 个」。

**每行左侧 38px 文件类型图标**：视频 / 音频 / 镜像 / 压缩包 / 失效。先看
扩展名，认不出来再看名字里有没有 `2160p`、`WEB-DL`、`x265` 这类标记 ——
多文件种子的名字是目录名，本来就没有扩展名。`icons.tsx` 里的 `iconKindFor`。

**底部状态栏**：任务数、全局速度、上传限速、DHT 节点数、监听端口、日志入口。
后两项来自 `session_status` 命令（`get_dht().stats().routing_table_size` 和
`listen_addr()`）。**没有「端口已映射」** —— librqbit 不暴露 UPnP 映射结果，
状态栏写一个猜的结论比不写更糟。

**状态用小圆点而不是填色胶囊**：胶囊在窄窗口里会被挤成两行（「做种/中」），
几个任务并排时还满屏色块。做种绿、下载蓝、暂停灰、出错红，进度条同色 ——
以前做种和下载都是同一个蓝胶囊，一眼分不出哪个还在耗带宽。

**行内操作是 24px 图标且平时隐形**，指到那一行才出现。三个描边按钮常驻会把
每行都塞满。删除确认仍然是文字按钮：三个图标分不清「仅移除任务」和「连同文件」。

**标题栏用 `titleBarStyle: "Overlay"` + `hiddenTitle`**，内容延伸到红绿灯
下面。侧边栏顶部的 `.titlebar-gap` 和工具栏的 32px 上内边距都是给这条留的，
**控件放进去点不动**。

这两块还必须标上 `data-tauri-drag-region`，否则**窗口拖不动** —— Overlay
之后系统标题栏被网页盖住，鼠标事件全被 webview 吃掉。属性只对事件目标本身
生效，所以下面的输入框和按钮照常可点。还要在 `capabilities/default.json` 里
加 `core:window:allow-start-dragging`，这条**不在 `core:default` 里**，漏了
就是标了属性也拖不动。

## 边下边播怎么工作

展开任务 → 媒体文件旁边会出现已安装播放器的按钮（IINA / VLC / mpv /
QuickTime）。点一下就 `open -a <播放器> <本地流地址>`。

流地址由 `stream_server.rs` 提供，`librqbit` 的 `FileStream` 在被读取时会把
该文件的分片提到最高优先级，并按「首片 → 尾片 → 中间顺序」抓，所以起播和
拖进度都不用等整个种子下完。实测在一个 3.4GB 的种子上直接跳到 1.7GB 处，
几秒内就能取到数据。

服务只绑 `127.0.0.1`，端口由系统分配，路径里带一次性随机 token —— 免得同机
其他程序能枚举、读取正在下载的内容。

也可以「复制链接」，粘到播放器的「打开网络串流」里。

## 已知取舍

- **任务目录靠自己记。** librqbit 把每个任务的输出目录存在 `pub(crate)` 字段里
  读不到（`ManagedTorrentShared.options` 整个是 `pub(crate)`），所以 Engine 自己
  记一份，写在配置目录的 `output_folders.json` 里。**按 info-hash 索引** ——
  TorrentId 是会话每次启动重新分配的，拿它当键重启后就对不上了。
- **自定义目录要多解析一次种子。** librqbit 只在使用会话默认目录时才自动建
  子目录（`session.rs` 里 `(Some(o), None) => PathBuf::from(o)` 把自定义目录
  原样拿去用），多文件种子会把几十个文件直接倒进目标目录。`Engine::add` 先用
  `list_only` 探一次拿到种子名和文件数，自己补上这一层。探测返回的
  `torrent_bytes` 会复用，所以磁力链不会重新解析元信息。
- **界面用 1 秒轮询**而不是事件推送。任务量大时可以改成 Rust 侧 emit 事件。
- **对话框不能挂在工具栏里。** 工具栏有 `backdrop-filter`，而**带
  `backdrop-filter` 的元素会成为固定定位后代的包含块** —— 对话框挂在
  `<header>` 里的话，`position: fixed` 的遮罩会被困在工具栏那一条里，整个
  错位。所以三个对话框都是 `<main>` 的直接子节点。以后加 `filter`、
  `transform` 到祖先元素上会踩同一个坑。
- **设置对话框自身滚动 + 操作栏 `sticky` 钉底。** 620pt 高的窗口里设置项比
  对话框还长，不钉的话「保存」会被挤出可视区，用户以为改完没法生效。
- **说明文字里的行内 `<b>` 曾经被打断。** `.setting b` 会连 `<em>` 里嵌套的
  强调一起变成块级，句子被拦腰截断、标点单独掉一行。现在收窄成
  `.setting > span > b`，只让标题独占一行。
- **设置图标是滑杆不是齿轮。** 16px 视口画不出齿，圆圈加八根辐条实机看着
  是个太阳。
- **`Speed.mbps` 实际单位是 MiB/s**，不是兆比特。`engine.rs` 里统一换算成字节/秒了。
- **播放器检测是硬编码的**：两个平台都是按固定名字查固定目录，见
  `platform.rs` 里的 `KNOWN`。装在别处就认不出来。
- **Windows 侧只做了类型检查，没有实机验证过。** `platform.rs` 的 Windows
  分支用 `cargo check --target x86_64-pc-windows-msvc` 验过能编译，但播放器
  路径、日志目录、阻止休眠的实际行为都还没在真机上跑过。
- **暂停中的任务不能起播。** 暂停状态不会有新数据进来，播放器只会卡住，
  所以按钮直接禁用，后端也会明确报错。
- **取消勾选不会删已下的数据。** `update_only_files` 只改 chunk tracker，
  不动磁盘上已有的文件。而且初始化中的任务改不了选择，librqbit 会直接报错。
- **预览缓存是一次性的。** `preview` 会把 `torrent_bytes` 连同算好的子目录
  缓存起来，`add_previewed` 取用后立刻删除 —— 这样磁力链只解析一次。最多存
  8 份，超了整个清掉（预览本来就是临时的）。取消添加会留下一份直到被挤掉。
- **移动之后就不做种了。** 文件一挪走 librqbit 就找不到它们，继续管着只会
  报错或触发重下。所以移动成功后会把任务从列表移除（文件保留）。
- **解压只支持 zip。** rar 和 7z 要靠外部工具，装没装不好说，不如不做。
  压缩包里的路径是外部输入，`safe_entry_path` 会挡掉 `../` 和绝对路径
  （zip slip），测试里用真实的恶意压缩包验过。
- **移动和解压都不覆盖已有内容**，撞名就加 ` (2)`、` (3)`。
- **RSS 自动添加走的是「整包下载」。** 自动触发时没人在旁边勾文件，
  所以不做预览、全部下载。需要挑文件的话在任务列表里展开改。
- **已知字段格式不对会让整份设置回到默认值**（包括下载目录）。未知字段和
  缺失字段都能正常容忍，只有类型不匹配才会这样。宁可回默认也不要半份配置。
- **用的是 9.0.0-rc.0，预发布版。** 为了 uTP 值得，但要有心理准备。
- **v9 用固定监听端口取代了 8.x 的端口范围。** 隔离实例（测试）必须用
  端口 0 让系统随机分配，否则并行跑测试会互相抢 4240。
- **公共 tracker 开关要重启才生效**：`SessionOptions.trackers` 只在建会话时
  读，而 v9 的 `AddTorrentOptions` 已经没有按任务设 tracker 的字段了。
- **只能跑一个实例。** BT 会话独占监听端口（DHT 持久化还会把端口钉死），
  两份一起跑既起不来也会互相写坏 session。第二次启动改成把已有窗口拉到前面。
  注意 `tauri dev` 重建时偶尔会留下孤儿进程，占着 4240 / 50522，
  下次启动就会被单实例挡掉 —— 用 `pkill -f target/debug/mydl` 清掉即可。
- **启动失败弹对话框而不是崩溃。** setup 钩子里把错误往上抛的话 Tauri 会
  `panic!`，用户看到的是系统的「意外退出」报告，完全看不出原因。
- **日志两层都要关 ANSI。** span 字段的格式化结果按 field-formatter 类型
  缓存在 span extensions 里，终端层和文件层共用 `DefaultFields` 就共用同一份
  缓存 —— 只在文件层 `with_ansi(false)` 无效，终端层先写进去的带色版本会被
  直接复用，日志文件里全是 `^[[3m` 之类的乱码。

## 路线图

- [x] 添加磁力链 / 种子文件、暂停继续、删除、进度显示、重启续传
- [x] 边下边播（文件列表 + 本地流媒体服务，支持拖进度）
- [x] 按文件勾选，只下需要的部分（添加时先预览，下载中也能改）
- [x] 完成后自动化（系统通知、解压 zip、移动到指定目录）
- [x] RSS 订阅按规则自动加种
- [x] 全局上传限速（运行时可改）
- [x] 界面重构：侧边分类栏、文件类型图标、底部状态栏

按规则重命名**没做**：想做好要解析剧集编号、季数、发布组等等，是一整套
匹配规则，没有明确需求的情况下做出来多半是猜错方向。真需要时再说。

## 完成后自动化

界面右上角「完成后处理」里配置，存在同一份 `settings.json`：

| 设置 | 默认 | 说明 |
|---|---|---|
| 下载完成时发系统通知 | 开 | 通知里会说明内容最终位置 |
| 自动解压 .zip | 关 | 解到同名子目录，原压缩包保留 |
| 完成后移动到指定目录 | 关 | **会停止做种**，见下 |

`automation.rs` 每 5 秒轮询一次，在「未完成 → 完成」的那一刻触发，
按 解压 → 移动 → 通知 的顺序执行。启动时已完成的任务不会触发，
否则每次开 App 都会把历史任务重播一遍。

## RSS 订阅

界面右上角「RSS」里管理。每条订阅有 包含 / 排除 两组关键词，空格分隔、
不区分大小写：包含的词要**全部**命中，排除的词命中**任一**即跳过。
「保存并立即检查」能马上看到每条订阅匹配了多少、新增了多少。

**用关键词而不是正则**是有意的：关键词写错顶多不匹配，正则写错可能匹配到
一切 —— 对一个会自动开始下载的功能来说，前者安全得多。

`rss.rs` 从条目里找下载地址的优先级是 磁力链 > 声明为 bittorrent 的
enclosure > 以 `.torrent` 结尾的链接。**找不到就跳过，不会退而求其次用
网页链接** —— 那多半是详情页，加进去只会报错。

已处理过的条目记在 `rss_seen.json`（每条订阅最多 500 个），否则重启后会把
整个订阅源重下一遍。光看「是否已在任务列表里」不够：任务完成后会被删掉或
移走。添加失败的条目不记录，下次检查会重试。

## 合规

客户端本身合法。不要内置指向盗版资源的索引或搜索。
