# mydl

一个自用的 BT 下载器，macOS 和 Windows 都能跑。Tauri 2 外壳 + React 界面，
协议栈用 [librqbit](https://github.com/ikatson/rqbit)（纯 Rust 实现，支持 uTP）。

它和别的下载器最大的不同在于：**卡住的时候它会告诉你为什么。** 0 peers 可能是
你按了暂停、可能是 BT 流量跟着 VPN 走了隧道、可能是这个资源真没人做种了 ——
这几件事在别的客户端里长得一模一样，而它们指向完全不同的处理办法。

> 开发文档（技术选型、实现细节、踩过的坑）在 **[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)**。

## 功能

**下载与播放**

- 磁力链 / 种子文件，暂停继续、删除、重启续传
- **边下边播**：文件列表里点一下就用 IINA / VLC / mpv / QuickTime 打开，
  支持拖进度，不用等下完（本地流媒体服务只绑 `127.0.0.1`，路径带一次性 token）
- 添加前预览文件列表，**只下需要的那几个**，下载中也能改
- 四条添加路径：粘贴、「打开种子…」、把 `.torrent` 拖进窗口、切回窗口时
  发现剪贴板里有磁力链就问你要不要加（绝不自动添加）

**找资源**

- 接你自己搭的 **Prowlarr / Jackett** 搜种子，可选让大模型按你的意图重排并给理由
- **RSS 订阅**按关键词自动加种，每条订阅可以单独指定保存目录
- **给已有任务找替代源**：解析压制名去索引器找同一部片子的其他版本，
  实查做种数排序 —— **只列不自动换**，换源意味着已下的字节全部作废

**看得懂的状态**

- **「为什么不动？」**：一键跑六步取证（暂停状态 → 网卡 → BT 实际走哪条路 →
  tracker 有没有源 → 能不能握手 → 拿官方种子做对照组），给一句确定的结论
- **swarm 健康度走势**：每半小时向公共 tracker scrape 一次，说得出
  「还在变好还是已经没人做了」，不是一个瞬时的 `seeders: 2`
- **还要多久**：按实测吞吐量算，还会说「卡了几小时」。**刻意不出完成概率** ——
  实测数据的变异系数在 34%~151%，报百分比是在编造精度
- **验伪**：读容器头（MP4 盒子 / MKV EBML / AVI / TS）对分辨率、音轨语言、
  码率，看看是不是名字里吹的那个 —— 不用解码器，取头部几 MB 就能验 14 GB 的种子
- **下载统计**：累计数字 + 按年分组的活动热力图
- **稀缺度**：做种行会说「全网只有 3 份在做种，你是其中之一」，
  工具栏那颗星把不稀缺的一次停掉

**自动化与省心**

- 完成后：系统通知、提示音、解压 zip、移动到指定目录、全部下完让电脑睡眠
- 下载期间不让电脑休眠（**做种不算**，没道理为了给别人上传整夜不睡）
- 并发下载上限自动排队、分享率到顶自动停做种
- **只恢复它自己暂停过的任务** —— 你手动暂停的不会被擅自放出来

**网络与隐私**

- **BT 绑定网卡**：开着全局 VPN 时让 BT 绕过隧道直出，而完全不动 VPN 本身。
  实测同一条种子 45 秒：走隧道 1 个 peer / 0.5 MB，绑物理网卡 62 个 peer / 588 MB
- 全局上传限速，以及**自适应限速**：ping 网关测排队延迟，
  只在「确实是我们自己把上行灌满了」的时候退让
- SOCKS5 代理、IP 黑名单、每任务 peer 上限、公共 tracker 开关
- 模型 API key 存**系统钥匙串 / 凭据管理器**，不落进配置文件

界面是深浅双主题（默认深色）、左侧分类栏 + 底部状态栏，状态栏常驻显示
DHT 节点数、监听端口和当前绑定的网卡 —— 网卡失效会立刻标红。

## 安装

去 [Releases](https://github.com/sunjianspss/mydl/releases) 下对应平台的包。
macOS 是 universal，Intel 机器也能跑。

**两个包都没有签名**（没买 Apple / 代码签名证书），所以第一次打开要多做一步。

### macOS

1. 双击 `.dmg`，把 mydl 拖进「应用程序」
2. 打开「终端」，粘贴这行回车：

   ```bash
   xattr -dr com.apple.quarantine /Applications/mydl.app
   ```

3. 再双击打开

**不要指望「系统设置 → 隐私与安全性」里的「仍要打开」按钮。** 那个按钮是给
「有签名但没公证」的 app 的；这个包连有效签名都没有，被拦时更可能直接说
「已损坏，你应该将它移到废纸篓」—— 那种情况下按钮根本不出现。上面那条
`xattr` 在哪个 macOS 版本上都稳。

### Windows

1. 跑 `.exe` 安装包，SmartScreen 拦下来时点「更多信息 → 仍要运行」
2. 首次启动时**防火墙会问是否允许 4240 端口，要点允许** —— 否则连不上 peer
3. Win10 还需要 [WebView2 运行时](https://developer.microsoft.com/microsoft-edge/webview2/)（Win11 自带）

### 首次启动会弹什么

提前知道一下，免得以为中毒了：

| 提示 | 要不要给 |
|---|---|
| 「mydl 想要接受传入网络连接」 | **必须允许**。BT 要监听端口收 peer，边下边播的本地服务也走这里，拒绝了速度会很惨 |
| 通知权限 | 可选，任务完成提醒 |
| 访问「下载」文件夹 | 默认下载目录取系统 `~/Downloads`，可以在设置里改到别处 |

### 从源码构建

需要 Rust、Node + pnpm（Windows 还要 VS Build Tools 的「使用 C++ 的桌面开发」）：

```bash
pnpm install
pnpm tauri build     # 产物在 src-tauri/target/release/bundle/
```

细节见[开发文档](docs/DEVELOPMENT.md#开发)。

## 搜索：接一个索引器，再（可选）配 AI

**这个项目不内置任何索引，也不生成任何 info-hash。** 搜索结果全部来自
**你自己搭的 Prowlarr 或 Jackett**（两者对外说的都是 Torznab 协议），
mydl 只是个客户端。没配索引器的话，搜索这一栏不可用。

### 第一步：跑一个 Jackett 或 Prowlarr

两个选哪个都行，Jackett 更轻、开箱能用，Prowlarr 界面更现代、适合和
*arr 全家桶一起用。用 Docker 最省事：

```bash
# Jackett
docker run -d --name jackett -p 9117:9117 \
  -v ~/jackett/config:/config \
  lscr.io/linuxserver/jackett:latest

# 或者 Prowlarr
docker run -d --name prowlarr -p 9696:9696 \
  -v ~/prowlarr/config:/config \
  lscr.io/linuxserver/prowlarr:latest
```

macOS 上不想装 Docker 的话，Jackett 也有原生包
（[官方 Releases](https://github.com/Jackett/Jackett/releases) 里的 macOS 版）。

跑起来之后打开 `http://localhost:9117`（Prowlarr 是 `http://localhost:9696`）。

### 第二步：加站点

在它的界面里点「+ Add indexer」，搜你要的站加进去。几条经验：

- **想用中文片名搜，就必须加中文站。** 只收录英文资源的站用中文名搜，
  正确结果是 **0 条**而不是一堆无关的 —— mydl 会自己筛掉不匹配的，
  否则有些索引器匹配不到时会把当季新片榜单原样返回，看起来就像「搜索坏了」。
- 加完每个站都点一下「Test」，登录失效或者站点改版的话这里会直接报错。
- 私有站（PT）要填账号 / passkey，这些都存在 Jackett/Prowlarr 那边，mydl 不碰。

### 第三步：把 Torznab 地址粘进 mydl

在 mydl 的**设置 → 搜索：索引器地址**里，把索引器界面上那条 Torznab 地址
**整条**粘进来（**含 `apikey`**）：

```
# Jackett，聚合所有站点的那条（界面上叫 "All Indexers" → Copy Torznab Feed）
http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab/api?apikey=你的key

# Prowlarr，聚合地址；也可以用某个单站的 "Copy Torznab Url" 再补上 ?apikey=
http://127.0.0.1:9696/api/v1/indexer/all/results/torznab/api?apikey=你的key
```

apikey 在 Jackett 界面右上角、Prowlarr 的 Settings → General 里。

**为什么要你整条粘、不拆成「地址 + API key」两个框**：Prowlarr 和 Jackett 的
路径前缀不一样，我们替你去拼必错，不如让你把界面上现成的那条复制过来。

配好之后工具栏的搜索就能用了。搜到的每条在加入任务前都会**真实探测一次
DHT/tracker**，探不到的会超时报错，不会默默加一个永远不动的任务。

### 第四步（可选）：配一个模型做 AI 排序

填了 API key 之后，索引器返回的结果会先送给模型按你的意图重排，
每条还会附一句理由（比如「这条是原盘 remux，体积大但没二压」）。

在**设置 → 搜索：AI 排序**里填两样东西：

| 填什么 | 说明 |
|---|---|
| API key | 存进**系统钥匙串**（Windows 是凭据管理器），不写进配置文件。存进去就读不回来，界面只显示「已保存到系统钥匙串」，想换就重填 |
| 模型 id | 默认 `deepseek-v4-flash`。换成你那家服务商的模型名即可 |

**换服务商要改配置文件。** 服务地址默认是 DeepSeek 的
`https://api.deepseek.com`，界面上没给输入框，改的话编辑配置目录里的
`settings.json`：

```jsonc
{
  "aiBaseUrl": "https://api.deepseek.com",   // 要求是 OpenAI 兼容的 /responses 端点
  "aiModel": "deepseek-v4-flash",
  "aiRank": true                             // 填了 key 就默认开；不想让模型掺和就改 false
}
```

请求打的是 `{aiBaseUrl}/responses`，也就是 **OpenAI Responses API 那套**，
不是老的 `/chat/completions`。填一个只兼容 chat completions 的地址会连不上。

**没有索引器、只填了 key 的话，搜索会退化成「让模型自己上网找」。**
这条路很弱 —— 通用搜索引擎基本索引不到种子站 —— 结果里每条都会标上
**「未经验证」**，能不能用要等加入任务时那次真实探测才知道。真想搜得准，
还是老老实实搭一个 Jackett。

### 模型碰不到的东西

**模型只能对索引器给的列表重新排序，不能产出链接。** 这不是提示词层面的
约定，是结构限制：拿到回复后只按**序号**从我们自己的列表里取对象，
输出里夹带的任何链接都会被丢掉，有测试钉着这条。

理由很硬：磁力链的 info-hash 是内容的 SHA-1 摘要，模型推导不出也记不住，
让它「给条磁力链」只会编出格式正确、DHT 里根本不存在的十六进制。

同理，**所有诊断结论的文案都写死在 Rust 里**，不交给模型 ——
「源太少」「swarm 已死」「是你的网络」这些判断是拿来做决定的，不能有发挥空间。

### 一条隐私提醒

模型的 API key 在钥匙串里，但**索引器地址里的 `apikey` 是明文存在
`settings.json` 里的**（因为那条地址必须整条粘贴，见上面）。

> **把 `settings.json` 发给别人排查问题之前，先把 `searchUrl` 里的
> `apikey=` 抹掉。**

风险本身不高（那是本机 Jackett 的 key，对方还得能访问你的 9117 端口），
但值得知道。

## 下不动的时候

按顺序试这三个，基本能定位到底是谁的问题：

1. **展开任务 → 「为什么不动？」**。它会真的发包做六步取证，跑 35~65 秒，
   最后给一句确定的结论（含一个拿官方种子跑的对照组 —— 没有对照组就分不清
   「这个资源不行」和「你的网络不行」，而这两者指向完全相反的处理）。
2. **开着 VPN / 全局代理的话，去设置里看「BT 走哪张网卡」**。TUN 模式下
   默认路由指向隧道，BT 跟着走隧道就基本废了：出口是机房 IP 会被大量客户端
   和 tracker 拒绝、没有入站连接所以做种完全无效。默认已经绑第一张物理网卡，
   但如果你手动改过「跟随系统默认路由」就会踩这条。
3. **展开任务看健康度那一行**。写着「0 个做种」并且连续几轮都是 0 的，
   就是真没人做了 —— 这时候用「找更好的源」去找同一部片子的其他版本。

## 文件都在哪

| | macOS | Windows |
|---|---|---|
| 配置和会话状态 | `~/Library/Application Support/com.sun.mydl/` | `%APPDATA%\com.sun.mydl\` |
| 日志（按天轮转，留 7 天） | `~/Library/Logs/mydl/` | `%LOCALAPPDATA%\mydl\logs\` |

界面右上角的「日志」按钮直接打开日志目录。想「恢复出厂」就删掉上面那个配置目录。

## 已知的边界

这是**自用级**软件，跑在 librqbit `9.0.0-rc.0` 预发布版上（为了 uTP 值得，
但要有心理准备）。另外几条常被问到的：

- **只能跑一个实例**，BT 会话独占监听端口；第二次启动会把已有窗口拉到前面。
- **移动到指定目录之后就不做种了** —— 文件一挪走 librqbit 就找不到它们。
- **分享率算的是装了这个版本之后的累计上传量**，不是种子的全部历史，
  和 PT 站上显示的对不上是正常的。
- **自动解压只支持 zip**，rar / 7z 要靠外部工具，装没装不好说，不如不做。
- **按规则重命名没做**，也没打算做 —— 没有明确需求的情况下做出来多半是猜错方向。

完整的取舍清单在[开发文档](docs/DEVELOPMENT.md#已知取舍)里，每一条都写了为什么。

## 开发

想改代码、了解某个功能为什么是这么实现的、或者自己发版打包，
看 **[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)**。

## 许可证

[MIT](LICENSE)。依赖各自的许可证见 `src-tauri/Cargo.toml` 和 `package.json` ——
librqbit 本身是 Apache-2.0。
