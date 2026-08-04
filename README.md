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

**在 Windows 上本地打包**（不支持从 macOS 交叉编译，Tauri 的 NSIS/MSI 需要
Windows 侧工具链）：

```powershell
# 先装 Rust、Node + pnpm，以及 VS Build Tools 的「使用 C++ 的桌面开发」
pnpm install
pnpm tauri build      # 产物在 src-tauri\target\release\bundle\nsis\
```

首次运行时 Windows 防火墙会问是否允许 4240 端口，**要点允许**，否则连不上 peer。
Win10 还需要 WebView2 运行时（Win11 自带）。

## 发版

打 `v` 开头的 tag，GitHub Actions 就同时出 macOS（universal，Intel 也能跑）
和 Windows 的安装包，收进一个**草稿 release**，自己看过再手动发布。

```bash
# 三处版本号必须一致，否则 CI 十秒内就会失败（不会白等十几分钟编译）
# src-tauri/tauri.conf.json、package.json、tag
git tag v0.2.0 && git push origin v0.2.0
```

想先验证 workflow 本身能不能跑通，去 Actions 页面手动触发（`workflow_dispatch`）
—— 不用为了试 CI 打一串废 tag。手动触发时跳过版本校验，只出产物不建 release。

**两个包都没有签名**（没买证书）：Windows 的 SmartScreen 要点「更多信息 →
仍要运行」；macOS 要用 `xattr` 摘掉 quarantine 标记，**别指望「仍要打开」
按钮**，见「分发」。

CI 里的 pnpm 大版本写死在 workflow 里，换开发机 pnpm 版本时记得同步，
否则 `--frozen-lockfile` 会因为 lockfile 格式差异跑挂。

## 分发

包出来之后怎么送到人手上。因为没签名，**不能指望对方双击就能装**。

### 为什么你自己跑得动，别人跑不动

不是因为"你是开发者"，也不是因为钥匙串里那张 `Apple Development` 证书 ——
那张证书全程没参与。`.app` 上的签名是链接器自动加的 ad-hoc：Apple Silicon
要求所有 arm64 二进制至少有 ad-hoc 签名才能执行，这跟身份无关。

真正的开关是 **`com.apple.quarantine` 扩展属性**。Safari、微信、AirDrop 这些
"下载方"在文件落盘时打这个标记，而 `tauri build` 在本地生成文件时不打。
Gatekeeper 只检查带标记的文件，所以本地产物根本没被问过。问它的话照样不合格：

```bash
spctl -a -vv src-tauri/target/release/bundle/macos/mydl.app
# rejected：code has no resources but signature indicates they must be present
```

反过来也成立：把 dmg 传上网盘，再用 Safari 下载回**同一台机器**，一样打不开。
想复现对方看到的效果，在副本上手动打标记：

```bash
cp -R src-tauri/target/release/bundle/macos/mydl.app /tmp/mydl-test.app
xattr -w com.apple.quarantine "0081;00000000;Safari;" /tmp/mydl-test.app
open /tmp/mydl-test.app     # 看完删掉，别在 bundle/ 里的原件上做
```

### 给 macOS 用户的说明（可以直接转发）

> 1. 双击 dmg，把 mydl 拖进"应用程序"
> 2. 打开"终端"，粘贴这行回车：
>    ```
>    xattr -dr com.apple.quarantine /Applications/mydl.app
>    ```
> 3. 再双击打开

**不要指望「系统设置 → 隐私与安全性」里的「仍要打开」按钮。** 那个按钮是给
"有签名但没公证"的 app 的；我们这个连有效签名都没有，被拦时更可能直接说
「已损坏，你应该将它移到废纸篓」—— 那种情况下按钮不出现。具体文案随 macOS
版本变，但 `xattr` 这条路在哪个版本上都稳。

代价是对方必须会开一次终端。想免掉这一步，只有买证书做签名 + 公证，见下。

### 首次启动会弹什么

提前打个招呼，免得对方以为中毒了：

- **「mydl 想要接受传入网络连接」** —— 必须允许。BT 要监听端口收 peer，
  边下边播的本地 HTTP 服务也走这里。拒绝了速度会很惨。
- **通知权限** —— 任务完成提醒，可选。
- **访问"下载"文件夹** —— 默认下载目录取系统 `~/Downloads`（`lib.rs` 的
  `init_app`），macOS 的 TCC 会问一次。可以在设置里改到别处。

Windows 侧对应的是 SmartScreen 和防火墙 4240 端口，见「Windows」。

### 对方出问题时管他要什么

日志在 `~/Library/Logs/mydl/`（Windows 见「Windows」的表），配置和会话状态在
`~/Library/Application Support/com.sun.mydl/` —— 要"恢复出厂"就删这个目录。

**让别人发配置过来之前提醒一句：`settings.json` 里的 `searchUrl` 带着索引器
的 apikey，先抹掉再发。** 模型的 API key 不在里面（在钥匙串里），不用管。

另外记得说明这是跑在 **librqbit 9.0.0-rc.0 预发布版**上的自用级软件，
见「已知取舍」。

### 想做到双击即用

买 Apple Developer Program（$99/年），建 `Developer ID Application` 证书
（`Apple Development` 那张不行，它不能用于对外分发），然后给 CI 加 secrets：

```
APPLE_CERTIFICATE / APPLE_CERTIFICATE_PASSWORD   # 导出的 .p12，base64
APPLE_SIGNING_IDENTITY                            # Developer ID Application: ...
APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID         # APPLE_PASSWORD 是 app 专用密码
```

`tauri-action` 认这几个环境变量（具体名字以它的文档为准），加上之后产物自带
hardened runtime 签名并送 Apple 公证，对方双击零提示。没做之前，上面那条
`xattr` 就是标准流程。

## 添加任务的几条路

粘贴到输入框、「打开种子…」选文件，另外两条：

**拖 .torrent 进窗口。** 只认文件 —— Tauri 接管了 webview 的原生拖放，回调里
拿到的是**文件路径**，从浏览器拖过来的磁力链是文本，根本走不到这个回调。
那条路交给剪贴板。

**切回窗口时检查剪贴板**（默认开，设置里可关）。剪贴板里有磁力链就显示一个
横幅问要不要添加，**绝不自动添加**。只在窗口重新获得焦点时读一次，不在后台
轮询：常驻读剪贴板既让人不安，macOS 15 起还会弹「某某读取了剪贴板」的系统
提示。已经在任务列表里的 info-hash 不会再问。

权限只申请了 `clipboard-manager:allow-read-text`，写剪贴板的权限没要。

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

正因为这份预算是全局共享的，做种的任务会把它吃光，下载中的任务就没有可回报
给对方的上行，容易被 choke 到零速 —— 表现是任务挂着几天不动。工具栏因此有
一个**「暂停做种」**按钮（`pause_seeding` 命令），只停 `live && finished` 的
任务，还在下的原样跑着。「全部暂停」在这种时候没用，它会把还在下的一起停掉。

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
| 待查有几个 peer 但仍超时 | swarm 太瘦。`read_metainfo_from_peer_receiver` 用 `seen` 集合保证每个地址只试一次，如果只有两三个 peer 且都不给元信息，就没有别的可试了 —— 这种情况确认一下公共 tracker 开关是开着的（默认开），只靠 DHT 找源太窄 |

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
├── AddDialog.tsx        添加前预览文件列表并勾选
├── SearchDialog.tsx     搜索种子 + 结果列表
├── SettingsDialog.tsx   设置
├── RssDialog.tsx        RSS 订阅管理
├── icons.tsx            内联 SVG 图标 + 按文件名判类型
├── theme.ts             深浅主题（localStorage，默认深色）
├── platform.ts          把平台写到 <html data-platform>
├── types.ts             与 Rust 侧对应的类型
└── format.ts            字节/速度格式化

src-tauri/src/
├── engine.rs            librqbit 会话的封装，不含 Tauri 类型
├── platform.rs          平台差异（日志目录、播放器、阻止休眠、提示音）
├── search.rs            Torznab 客户端（Prowlarr / Jackett）
├── ai.rs                大模型排序；只认序号，不接受它产出的链接
├── secrets.rs           API key 存系统钥匙串 / 凭据管理器
├── rss.rs               RSS 订阅按关键词自动加种
├── automation.rs        完成后：通知、解压、移动
├── keep_awake.rs        下载期间阻止休眠
├── stream_server.rs     本地 HTTP 流媒体服务（Range 支持）
├── settings.rs          持久化设置（JSON，原子写）
└── lib.rs               Tauri 命令 + 应用入口

src-tauri/tests/
├── engine_smoke.rs      子目录、只下选中文件、取消预览、重启后记得目录
├── torznab_real.rs      拿真实 Jackett 响应验解析（fixture 已脱敏）
├── stream_smoke.rs      边下边播 + Range（联网，默认跳过）
└── magnet_diag.rs       磁力链诊断工具（手动跑）
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

**状态圆点会往外扩散**，像雷达在扫。只有下载中和做种中会动（下载快、做种慢），
暂停和出错保持静止 —— 全都在动的话「有没有在动」就没有信息量了。系统开了
「减弱动态效果」就自动关掉。

**行内操作是 24px 图标且平时隐形**，指到那一行才出现。三个描边按钮常驻会把
每行都塞满。删除确认仍然是文字按钮：三个图标分不清「仅移除任务」和「连同文件」。

**深浅主题一键切换**，工具栏右侧那个太阳/月亮图标，**默认深色**。选择存在
localStorage 而不是 `settings.json`：主题必须在首帧之前**同步**定下来，走 Rust
命令的话会先按默认色渲染一帧再跳，能看见闪白。代价是这项设置不跟着
`settings.json` 走，换机器要重设一次 —— 对一个显示偏好来说划算。

手动选过之后就**不再跟随系统外观**（`:root[data-theme]` 会盖掉
`prefers-color-scheme`）。`:root:not([data-theme])` 那条规则只是兜底：万一
脚本没跑到，还能跟随系统。别忘了 `color-scheme` 也要跟着切，否则复选框、
输入框、滚动条这些原生控件在深色下会是白底。

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
- **`.main` 和 `.list` 必须写 `min-height: 0`。** `.main` 是网格项、`.list` 是
  flex 项，两者默认都是 `min-height: auto`，不会收缩到内容以下，于是
  `overflow-y: auto` 形同虚设 —— 内容被 `.app` 的 `overflow: hidden` 裁掉，
  列表长了既滚不动也看不见。以后再往里塞可滚动的容器要记得这一条。
- **Windows 拉起播放器必须用 `spawn` 而不是 `status`。** 那边是直接跑播放器
  本体，`status` 会等到播放器退出；而 `open_in_player` 是同步 Tauri 命令、
  跑在主线程上，界面会卡死到用户关掉播放器为止。macOS 碰不到这个坑，
  `open -a` 交给 LaunchServices 后立刻返回。
- **启动时会把文件描述符软限制抬到 8192。** macOS 给 GUI 应用的软限制只有
  **256**（`launchctl limit maxfiles`），而 BT 是一个 peer 一个 socket，任务
  一多就撞上。症状极具误导性：种子照常下（那些 socket 早建好了），但边下边播
  的 HTTP 服务 `accept` 不了新连接，看起来像「播放坏了」，日志里是
  `axum::serve::listener: accept error: Too many open files`。
  **这个 bug 用 `pnpm tauri dev` 复现不了** —— dev 启动的进程继承终端的
  `ulimit`（通常上百万），只有 launchd 启动的安装版才是 256。
- **界面用 1 秒轮询**而不是事件推送。任务量大时可以改成 Rust 侧 emit 事件。
- **解析磁力链时可以主动取消**，不用干等满 120 秒。`Engine::preview` 用
  `tokio::select!` 在 probe 和一个 `Notify` 之间二选一，取消那条被选中时
  probe 那个 future 就被 drop —— librqbit 那边的解析随之取消，和超时走的
  是同一条路。取消返回 `Ok(None)` 而不是 `Err`：那不是错误，界面不该弹红条。
  用 `notify_one` 而不是 `notify_waiters`，后者在还没有人等待时会把通知丢掉，
  用户手快就会点了没反应。
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
- **Windows 已实机验过**：下载、播放器检测、日志目录、`SetThreadExecutionState`
  阻止休眠都正常。播放器路径仍是硬编码的猜测（见 `platform.rs` 的 `KNOWN`），
  装在别处照样认不出来 —— 只是说明常见安装位置猜对了。
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
  读，而 v9 的 `AddTorrentOptions` 已经没有按任务设 tracker 的字段了。好处是
  它是**会话级**的，打开后对已经在列表里的老任务一样生效，不用重新添加。
  另外改默认值只影响没写过配置的新安装 —— `settings.json` 里已经存了
  `usePublicTrackers: false` 的老用户得自己去设置里勾上。
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
- [x] 深浅主题一键切换（默认深色）
- [x] Windows 支持 + CI 出双平台安装包
- [x] 搜索种子（Torznab 索引器 + 大模型排序）
- [x] 拖 .torrent 进窗口、切回窗口时检查剪贴板
- [x] 任务完成提示音、解析磁力链可取消
- [x] SOCKS5 代理、IP 黑名单、peer 上限、下载限速
- [x] 分享率上限自动停做种、全部下完自动睡眠
- [x] 并发下载上限（自动排队）、全部暂停/继续、单独暂停做种

按规则重命名**没做**：想做好要解析剧集编号、季数、发布组等等，是一整套
匹配规则，没有明确需求的情况下做出来多半是猜错方向。真需要时再说。

## 完成后自动化

界面右上角「完成后处理」里配置，存在同一份 `settings.json`：

| 设置 | 默认 | 说明 |
|---|---|---|
| 下载完成时发系统通知 | 开 | 通知里会说明内容最终位置 |
| 下载完成时播提示音 | 开 | 见下，和通知是两件事 |
| 分享率到顶就停止做种 | 关 | 见下，**重启会归零** |
| 全部下完后让电脑睡眠 | 关 | 只在有任务真的完成那一轮触发 |
| 自动解压 .zip | 关 | 解到同名子目录，原压缩包保留 |
| 完成后移动到指定目录 | 关 | **会停止做种**，见下 |

**提示音和通知是分开的两项。** 通知权限被拒、或者开了勿扰，系统通知就不会
响；提示音是 App 自己播的，照样听得见。反过来也可以只要通知不要声音。用的都是
系统自带音效，不额外打包音频文件：macOS 是 `afplay` 放 `Glass.aiff`，Windows 是
`MessageBeep(MB_ICONASTERISK)`（用户在「声音设置」里换过就跟着换）。设置里有
「试听」按钮，不用等任务下完。

macOS 那边单独起线程等 `afplay` 结束：直接 spawn 不 wait 会留一串僵尸进程，
而在轮询循环里同步 wait 又会把循环卡住一秒。

**并发上限只恢复它自己暂停过的任务。** 超出上限时暂停 id 最大的（最后加进来
的），保证先来的先下完；名额空出来再按顺序放行。做种不占名额。关键是那份
「我暂停过谁」的记录只在内存里 —— 重启后排队中的任务会停在暂停状态等你手动
继续。宁可这样，也不能擅自恢复你手动暂停的任务。判定逻辑在 `plan_concurrency`，
有单元测试钉着。

**分享率上限每次重启会归零。** librqbit 的 `uploaded_bytes` 只统计本次会话，
不持久化（`torrent_state/mod.rs` 里初始化为 0，会话文件里也没这个字段）。
所以「停在 2.0」的实际含义是「本次运行期间上传到文件大小的两倍」，
不是 PT 站看到的那个累计分享率 —— 想靠它刷分享率会失望。

**自动睡眠只在「这一轮真的有任务完成、且完成后一个未完成的都不剩」时触发。**
不这么限制的话，睡醒之后条件依然成立，会立刻又睡回去。睡前等 20 秒：给通知
留时间，也等 `keep_awake` 松开 `caffeinate`（它 15 秒才检查一次）；等待期间
有新任务进来就取消。macOS 走 `pmset sleepnow`，Windows 走 `SetSuspendState`。

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

## 搜索

搜索结果来自**你自己搭的 Prowlarr 或 Jackett**（两者说的都是 Torznab），
这个项目不内置任何索引，也不生成任何 info-hash。在设置里把它们界面上那条
Torznab 地址整条粘进来即可 —— 不拆成「地址 + key」是因为两家的路径前缀不同，
我们去拼必错。

填了 API key 的话，模型会把结果按你的意图重排并给一句理由。**模型只能对
索引器给的列表重新排序，不能产出链接**：`ai.rs` 拿到回复后只按序号从我们
自己的列表里取对象，输出里夹带的任何链接都会被丢掉。这不是提示词约定，
是结构限制，`model_output_cannot_inject_links` 那个测试钉着它。

理由很硬：磁力链的 info-hash 是内容的 SHA-1 摘要，模型推导不出也记不住，
让它「给条磁力链」只会编出格式正确、DHT 里根本不存在的十六进制。

无论链接来自索引器还是模型排序，加入任务前都要走 `preview_torrent` 真实
探测一次 DHT/tracker —— 编造的 hash 在那一步必然超时暴露。

**索引器给的 `.torrent` 地址可能 302 跳到磁力链。** Jackett 的 `/dl/…` 端点
对「只给磁力链的站」（LimeTorrents 之类）是个跳板，会重定向到 `magnet:`。
librqbit 拿到 302 直接报错 —— 通用 HTTP 客户端确实不该跟到非 HTTP 协议上去。
所以 `preview` 之前先用 `resolve_uri` 自己解一次：跳到磁力链就改用磁力链，
跳到别的 http 地址（多半是真的 .torrent）或者不是重定向就原样交给 librqbit。

**拿到结果后还会自己筛一道相关性。** 有些索引器匹配不到时会返回自己的默认
榜单 —— 实测 The Pirate Bay 搜「指环王」会回 100 条当季新片，用户看到的就是
「搜索坏了」。`matches_query` 的规则和 RSS 订阅的「包含」一致：关键词全部命中
才留下，按空白分词，单个拉丁字母忽略（`a` 匹配一切），中日韩不分词。

所以用中文片名搜只收录英文资源的站，正确结果是 **0 条**而不是 100 条无关的。
想搜中文就在 Jackett 里加中文站。

模型的 API key 存**系统钥匙串**（macOS）/ **凭据管理器**（Windows），不落进
settings.json：那个文件是明文的，而下面「分发」一节还在教人去翻那个目录排查
问题。存进去就读不回来，界面只显示「已保存」，想换就重填。

**但索引器地址里的 apikey 仍然是明文存在 settings.json 里的。** 没跟着挪进
钥匙串，是因为那条地址必须整条粘贴 —— Prowlarr 和 Jackett 的路径前缀不一样，
拆成「地址 + key」两个字段我们就得去猜怎么拼，必错。所以：

> **把 settings.json 发给别人之前，先把 `searchUrl` 里的 `apikey=` 抹掉。**

风险本身不高（那是本机 Jackett 的 key，对方还得能访问你的 9117 端口），
但值得知道。同理，**抓 Torznab 响应当测试样本时也要先脱敏** —— 响应里每条
结果的链接都带着 apikey，`tests/fixtures/jackett_lotr.xml` 就是这么脱敏过的。
