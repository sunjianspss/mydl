//! 平台差异全集中在这里，别的模块里不该再出现 `cfg(target_os)`。
//!
//! 目前只支持 macOS 和 Windows。加平台就在下面补一个 `mod imp`。

#[cfg(not(any(target_os = "macos", windows)))]
compile_error!("mydl 目前只支持 macOS 和 Windows：日志目录、播放器检测、阻止休眠都要按平台实现");

// 前端判断平台走 userAgent（见 src/platform.ts）：那个判断必须同步拿到，
// 否则会先按 macOS 渲染出标题栏留白再跳掉。所以这里不需要导出平台名。

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use anyhow::{bail, Context, Result};

    pub fn home() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    pub fn log_dir() -> PathBuf {
        home().join("Library/Logs/mydl")
    }

    /// macOS 上常见的播放器。只返回真正装了的，界面按这个渲染按钮。
    const KNOWN: &[&str] = &["IINA", "VLC", "mpv", "QuickTime Player"];

    pub fn available_players() -> Vec<String> {
        let roots = [
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
            home().join("Applications"),
        ];

        KNOWN
            .iter()
            .filter(|name| {
                roots
                    .iter()
                    .any(|root| root.join(format!("{name}.app")).exists())
            })
            .map(|s| s.to_string())
            .collect()
    }

    /// 播放器认得的容器，`None` 表示什么都能放。
    ///
    /// IINA / VLC / mpv 自带解复用器，容器不是它们的限制，所以只有
    /// QuickTime 需要挑出来：它只认 ISO BMFF 那一支，mkv 打开是
    /// 「不支持的文件格式」。而 QuickTime 在 `/System/Applications` 里
    /// **必然存在** —— 不限制的话，每个 mkv 后面都会挂一个点了必然失败的
    /// 按钮，而 mkv 恰恰是影视资源的主流容器。对刚拿到软件的人来说，
    /// 那看起来就是「这软件的播放坏了」。
    ///
    /// 宁可少给按钮：漏掉一个其实能放的容器，代价是他多点一次「复制链接」；
    /// 多给一个放不了的，代价是他以为软件坏了。
    pub fn player_containers(name: &str) -> Option<&'static [&'static str]> {
        match name {
            "QuickTime Player" => Some(&[".mp4", ".m4v", ".mov"]),
            _ => None,
        }
    }

    /// 走 `open -a`，因为 http:// 交给系统默认处理会进浏览器。
    pub fn open_in_player(url: &str, app: &str) -> Result<()> {
        let status = Command::new("/usr/bin/open")
            .args(["-a", app, url])
            .status()
            .with_context(|| format!("启动 {app} 失败"))?;

        if !status.success() {
            bail!("{app} 退出码 {status}");
        }
        Ok(())
    }

    /// 用用户指定的路径拉起播放器。
    ///
    /// 两种都得认：`.app` 包走 `open -a`（它同样接受完整路径），其余当成
    /// 可执行文件直接跑 —— brew 装的 mpv 就在 `/opt/homebrew/bin/mpv`，
    /// 压根不是 `.app`，而那恰恰是自动检测认不出、需要手填的典型情况。
    ///
    /// 直接跑可执行文件时**必须 `spawn` 不能 `status`**：这条路径上的
    /// Tauri 命令是同步的、跑在主线程上，等播放器退出等于把界面冻住。
    /// `open -a` 没这个问题，它交给 LaunchServices 后立刻返回。
    pub fn open_path(url: &str, path: &str) -> Result<()> {
        let exe = PathBuf::from(path);
        if exe.extension().is_some_and(|e| e == "app") {
            return open_in_player(url, path);
        }
        Command::new(&exe)
            .arg(url)
            .spawn()
            .with_context(|| format!("启动 {path} 失败"))?;
        Ok(())
    }

    /// 任务完成的提示音。用系统自带的 Glass，不额外打包音频资源。
    ///
    /// 单独起线程等它结束：直接 spawn 不 wait 会留一串僵尸进程，而在调用方
    /// 那边同步 wait 又会把轮询循环卡住一秒。
    pub fn play_done_sound() {
        std::thread::spawn(|| {
            let r = Command::new("/usr/bin/afplay")
                .arg("/System/Library/Sounds/Glass.aiff")
                .status();
            if let Err(e) = r {
                tracing::warn!("播放提示音失败：{e}");
            }
        });
    }

    /// 想要的文件描述符上限。
    ///
    /// BT 是一个 peer 一个 socket，几百个连接很正常。macOS 给 GUI 应用的
    /// 软限制只有 **256**（`launchctl limit maxfiles`），撞上之后表现极具
    /// 误导性：种子照常下（那些 socket 早就建好了），但边下边播的 HTTP
    /// 服务 accept 不了新连接，看起来像「播放功能坏了」。
    ///
    /// 系统硬上限是 `kern.maxfilesperproc`（本机 184320），8192 足够用，
    /// 也不至于夸张到把系统资源占光。
    const WANT_FDS: libc::rlim_t = 8192;

    /// 启动时把文件描述符软限制抬上去。
    ///
    /// 失败只警告不中断 —— 限制没抬上去顶多是并发多了会出问题，不该因此
    /// 起不来。
    pub fn raise_file_limit() {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: 传的是本地变量的可变指针，长度由类型保证。
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } != 0 {
            tracing::warn!("读取文件描述符上限失败");
            return;
        }

        let old = lim.rlim_cur;
        // 硬上限可能是 RLIM_INFINITY，那时直接用我们想要的值 —— 传
        // RLIM_INFINITY 给 setrlimit 在 macOS 上会被拒。
        let target = if lim.rlim_max == libc::RLIM_INFINITY {
            WANT_FDS
        } else {
            WANT_FDS.min(lim.rlim_max)
        };

        if old >= target {
            tracing::info!(软限制 = old, "文件描述符上限够用，不动");
            return;
        }

        lim.rlim_cur = target;
        // SAFETY: 同上。
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) } == 0 {
            tracing::info!(原来 = old, 现在 = target, "已抬高文件描述符上限");
        } else {
            tracing::warn!(原来 = old, 想要 = target, "抬高文件描述符上限失败");
        }
    }

    /// 立刻让电脑睡眠。
    ///
    /// `pmset sleepnow` 是系统自带的做法，等价于菜单里点「睡眠」。
    pub fn sleep_now() -> Result<()> {
        let status = Command::new("/usr/bin/pmset")
            .arg("sleepnow")
            .status()
            .context("执行 pmset sleepnow 失败")?;
        if !status.success() {
            bail!("pmset 退出码 {status}");
        }
        Ok(())
    }

    /// 阻止休眠：拉一个 `caffeinate` 子进程。
    ///
    /// 不直接调 IOKit 是为了少一层 FFI，而且 `pmset -g assertions` 里能看到
    /// 是谁在阻止休眠。关键是 `-w <自己的 pid>`：万一 App 被强杀、来不及
    /// kill 子进程，caffeinate 也会跟着退出，不会留一个进程让电脑永远睡不着。
    #[derive(Default)]
    pub struct SleepBlocker {
        child: Option<std::process::Child>,
    }

    impl SleepBlocker {
        pub fn set(&mut self, want: bool) {
            match (want, self.child.as_mut()) {
                (true, None) => {
                    // -i 禁止闲置休眠，-m 禁止磁盘休眠，-s 禁止系统休眠（仅接电源时有效）
                    let spawned = Command::new("/usr/bin/caffeinate")
                        .args(["-i", "-m", "-s", "-w", &std::process::id().to_string()])
                        .spawn();
                    match spawned {
                        Ok(child) => {
                            tracing::info!(pid = child.id(), "有任务在下载，已阻止休眠");
                            self.child = Some(child);
                        }
                        Err(e) => tracing::warn!("启动 caffeinate 失败：{e}"),
                    }
                }
                (false, Some(child)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::info!("没有正在下载的任务，已解除阻止休眠");
                    self.child = None;
                }
                // 子进程意外没了（比如被人手动 kill），下一轮会重新拉起。
                (true, Some(child)) => {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        tracing::warn!("caffeinate 意外退出，将重新启动");
                        self.child = None;
                    }
                }
                (false, None) => {}
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use std::process::Command;

    use anyhow::{bail, Context, Result};
    use windows_sys::Win32::System::Power::{
        SetSuspendState, SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };
    // MessageBeep 在 Diagnostics::Debug 下（windows-sys 的元数据分组就是这么怪），
    // 常量却在 WindowsAndMessaging 里 —— 两个 feature 都得开。
    use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;

    pub fn home() -> PathBuf {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\"))
    }

    /// `%LOCALAPPDATA%\mydl\logs`。LOCALAPPDATA 拿不到时退回用户目录，
    /// 总比写不出日志强。
    pub fn log_dir() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join("AppData\\Local"))
            .join("mydl\\logs")
    }

    /// Windows 上常见的播放器：显示名 → 相对安装根目录的可执行文件路径。
    /// 一个播放器可能有多个候选路径（32/64 位、不同版本装到不同地方）。
    const KNOWN: &[(&str, &[&str])] = &[
        ("VLC", &[r"VideoLAN\VLC\vlc.exe"]),
        ("mpv", &[r"mpv\mpv.exe"]),
        (
            "PotPlayer",
            &[
                r"DAUM\PotPlayer\PotPlayerMini64.exe",
                r"DAUM\PotPlayer\PotPlayer.exe",
            ],
        ),
        (
            "MPC-HC",
            &[r"MPC-HC\mpc-hc64.exe", r"MPC-HC64\mpc-hc64.exe"],
        ),
    ];

    /// 装在哪都有可能：64 位和 32 位的 Program Files，以及只给当前用户装的。
    fn roots() -> Vec<PathBuf> {
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|k| std::env::var_os(k).map(PathBuf::from))
            .chain(std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs")))
            .collect()
    }

    fn find(app: &str) -> Option<PathBuf> {
        let (_, candidates) = KNOWN.iter().find(|(name, _)| *name == app)?;
        roots().iter().find_map(|root| {
            candidates
                .iter()
                .map(|rel| root.join(rel))
                .find(|p| p.exists())
        })
    }

    pub fn available_players() -> Vec<String> {
        KNOWN
            .iter()
            .filter(|(name, _)| find(name).is_some())
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Windows 这边 `KNOWN` 里的四个都自带解复用器，没有容器限制 ——
    /// 系统也不预装任何一个，不存在 macOS 上 QuickTime 那种「必然在、
    /// 但放不了 mkv」的情况。语义见 macOS 侧的同名函数。
    pub fn player_containers(_name: &str) -> Option<&'static [&'static str]> {
        None
    }

    /// 直接把流地址作为参数拉起播放器。不走 `cmd /c start`：那样 http://
    /// 会被交给系统默认程序，也就是浏览器。
    ///
    /// **必须用 `spawn` 而不是 `status`。** 这里是直接拉起播放器本体，`status`
    /// 会一直等到播放器退出；而这个 Tauri 命令是同步的、跑在主线程上，于是
    /// 整个界面卡死到用户关掉播放器为止 —— 看起来就是「点了播放没反应」。
    /// macOS 那边碰不到这个坑，`open -a` 交给 LaunchServices 后立刻就返回了。
    ///
    /// 代价是拿不到播放器的退出码，但那本来也没用：我们只关心有没有拉起来。
    pub fn open_in_player(url: &str, app: &str) -> Result<()> {
        let exe = find(app).with_context(|| format!("没找到 {app} 的安装位置"))?;
        Command::new(&exe)
            .arg(url)
            .spawn()
            .with_context(|| format!("启动 {app} 失败"))?;
        Ok(())
    }

    /// 用用户指定的路径拉起播放器。和 `open_in_player` 的区别只是跳过查找，
    /// 别的一样 —— 包括**必须 `spawn`**，理由见上。
    pub fn open_path(url: &str, path: &str) -> Result<()> {
        Command::new(path)
            .arg(url)
            .spawn()
            .with_context(|| format!("启动 {path} 失败"))?;
        Ok(())
    }

    /// 任务完成的提示音。走 `MessageBeep`，声音是系统「星号」提示音，
    /// 用户在「声音设置」里换过就跟着换 —— 比硬塞一个 wav 得体。
    /// 本身就是异步返回的，不用起线程。
    pub fn play_done_sound() {
        if unsafe { MessageBeep(MB_ICONASTERISK) } == 0 {
            tracing::warn!("播放提示音失败");
        }
    }

    /// Windows 不是 fd 模型，句柄上限由系统动态管理，没有对应操作。
    pub fn raise_file_limit() {}

    /// 立刻让电脑睡眠。
    ///
    /// 第一个参数 false = 睡眠而不是休眠（hibernate）；第二个 false = 不强制，
    /// 尊重那些正在阻止睡眠的程序 —— 强制睡下去可能打断别人正在写盘的活。
    pub fn sleep_now() -> Result<()> {
        // SAFETY: 三个都是纯值参数，没有指针。
        let ok = unsafe { SetSuspendState(0, 0, 0) };
        if ok == 0 {
            bail!("SetSuspendState 调用失败");
        }
        Ok(())
    }

    /// 阻止休眠：`SetThreadExecutionState`。
    ///
    /// 只挡系统休眠（ES_SYSTEM_REQUIRED），不挡息屏 —— 下载中没道理让显示器
    /// 一直亮着，这一点和 macOS 那边 `caffeinate -i -m -s` 的取舍一致。
    ///
    /// **这个状态是按线程记的**，所以调用方必须保证所有调用都在同一个长期
    /// 存活的线程上，见 `keep_awake::spawn`。
    #[derive(Default)]
    pub struct SleepBlocker {
        active: bool,
    }

    impl SleepBlocker {
        pub fn set(&mut self, want: bool) {
            if want == self.active {
                return;
            }

            let flags = if want {
                ES_CONTINUOUS | ES_SYSTEM_REQUIRED
            } else {
                // 只留 ES_CONTINUOUS 就是清掉之前设的那些要求。
                ES_CONTINUOUS
            };

            // 返回 0 表示失败；除了记一笔没别的可做。
            if unsafe { SetThreadExecutionState(flags) } == 0 {
                tracing::warn!("SetThreadExecutionState 失败，休眠状态未改变");
                return;
            }

            self.active = want;
            if want {
                tracing::info!("有任务在下载，已阻止休眠");
            } else {
                tracing::info!("没有正在下载的任务，已解除阻止休眠");
            }
        }
    }
}

pub use imp::{
    available_players, log_dir, open_in_player, open_path, play_done_sound, player_containers,
    raise_file_limit, sleep_now, SleepBlocker,
};

/// 放在顶层而不是 `imp` 里面：嵌在 `#[cfg(target_os = "macos")] mod imp`
/// 里的话，Windows 上整块被 cfg 掉，那支 `player_containers` 一条断言都跑不到
/// —— 而 CI 是出双平台包的。
#[cfg(test)]
mod tests {
    use super::player_containers;

    /// 自带解复用器的那些，两个平台都不该被限制。
    #[test]
    fn full_featured_players_are_unrestricted() {
        for name in ["IINA", "VLC", "mpv", "PotPlayer", "MPC-HC"] {
            assert!(player_containers(name).is_none(), "{name} 不该被限制");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quicktime_refuses_mkv_and_friends() {
        let q = player_containers("QuickTime Player").expect("QuickTime 必须受限");
        for ext in [".mkv", ".webm", ".avi", ".ts", ".m2ts", ".wmv", ".flv"] {
            assert!(!q.contains(&ext), "QuickTime 放不了 {ext}，不该出按钮");
        }
        assert!(q.contains(&".mp4"));
    }

    /// Windows 侧目前一个受限播放器都没有。将来谁加了限制，这条会红 ——
    /// 那时候前端的「都放不了」那个分支才第一次可达，记得一起看。
    #[cfg(windows)]
    #[test]
    fn windows_restricts_nothing_yet() {
        for name in ["VLC", "mpv", "PotPlayer", "MPC-HC", "QuickTime Player"] {
            assert!(player_containers(name).is_none());
        }
    }
}
