//! 完成后自动化：轮询任务状态，在「未完成 → 完成」的那一刻按设置执行动作。
//!
//! 三个动作按这个顺序跑：解压 → 移动 → 通知。通知放最后，这样消息里能报出
//! 内容的最终位置。任何一步失败都只记日志并在通知里说明，不影响其他任务。

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::engine::{Engine, TorrentId};
use crate::settings::SettingsStore;

/// 轮询间隔。完成后自动化不需要秒级实时。
const POLL: Duration = Duration::from_secs(5);

pub fn spawn(app: AppHandle, engine: Arc<Engine>, store: Arc<SettingsStore>) {
    tauri::async_runtime::spawn(async move {
        // 启动时已经完成的任务不触发 —— 否则每次开 App 都会把历史任务重播一遍。
        let mut done: HashSet<TorrentId> = engine
            .list()
            .into_iter()
            .filter(|t| t.finished)
            .map(|t| t.id)
            .collect();
        tracing::info!(已完成 = done.len(), "完成后自动化已启动");

        loop {
            tokio::time::sleep(POLL).await;

            for t in engine.list() {
                if !t.finished {
                    // 重新校验或补下时会退回未完成，这样下次完成还能再触发。
                    done.remove(&t.id);
                    continue;
                }
                if !done.insert(t.id) {
                    continue;
                }

                let settings = store.get();
                if !settings.notify_on_complete
                    && settings.move_to.is_none()
                    && !settings.extract_archives
                {
                    continue;
                }

                tracing::info!(id = t.id, name = %t.name, "任务完成，执行自动化");
                let outcome = run_actions(&engine, &store, t.id).await;

                if settings.notify_on_complete {
                    let (title, body) = match &outcome {
                        Ok(summary) => ("下载完成", format!("{}\n{}", t.name, summary)),
                        Err(e) => ("下载完成，但后续处理出错", format!("{}\n{e:#}", t.name)),
                    };
                    if let Err(e) = app
                        .notification()
                        .builder()
                        .title(title)
                        .body(body)
                        .show()
                    {
                        tracing::warn!("发送通知失败：{e:#}");
                    }
                }

                if let Err(e) = outcome {
                    tracing::error!(id = t.id, "自动化失败：{e:#}");
                }
            }
        }
    });
}

/// 返回一句给通知用的说明。
async fn run_actions(engine: &Arc<Engine>, store: &SettingsStore, id: TorrentId) -> Result<String> {
    let settings = store.get();
    let mut path = engine.output_path(id)?;
    let mut notes: Vec<String> = Vec::new();

    if settings.extract_archives {
        let n = extract_zips(&path).context("解压失败")?;
        if n > 0 {
            notes.push(format!("解压了 {n} 个压缩包"));
        }
    }

    if let Some(dest) = settings.move_to.as_deref() {
        let dest = Path::new(dest);
        // 已经在目标目录里就别折腾了。
        if path.parent() != Some(dest) {
            let moved = move_into(&path, dest).context("移动失败")?;
            notes.push(format!("已移动到 {}", moved.display()));
            path = moved;

            // 文件不在原处了，librqbit 再管着它只会报错或触发重下。
            // 从会话移除，但保留文件。
            engine
                .delete(id, false)
                .await
                .context("移动后从任务列表移除失败")?;
            notes.push("已停止做种".into());
        }
    }

    if notes.is_empty() {
        notes.push(format!("保存在 {}", path.display()));
    }
    Ok(notes.join("；"))
}

// ---------------------------------------------------------------------------
// 移动
// ---------------------------------------------------------------------------

/// 把 `src` 整个搬进 `dest_dir`，返回搬完之后的路径。
fn move_into(src: &Path, dest_dir: &Path) -> Result<PathBuf> {
    if !src.exists() {
        bail!("源路径不存在：{}", src.display());
    }
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("无法创建目标目录 {}", dest_dir.display()))?;

    let name = src
        .file_name()
        .context("源路径没有文件名")?
        .to_os_string();
    let target = unique_path(dest_dir, &name);

    // 同一个卷上 rename 是原子的、瞬间完成；跨卷会失败，退回复制+删除。
    match std::fs::rename(src, &target) {
        Ok(()) => Ok(target),
        Err(_) => {
            copy_recursive(src, &target)
                .with_context(|| format!("跨卷复制到 {} 失败", target.display()))?;
            remove_recursive(src)?;
            Ok(target)
        }
    }
}

/// 目标已存在就加 " (2)"、" (3)"…，绝不覆盖已有内容。
fn unique_path(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }

    let name = Path::new(name);
    let stem = name.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = name.extension().map(|e| e.to_string_lossy().into_owned());

    for n in 2..10_000 {
        let candidate = match &ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    // 实在撞满了就退回原名，让上层的 rename 报错，也好过覆盖。
    first
}

fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    if src.is_file() {
        std::fs::copy(src, dst)?;
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
    }
    Ok(())
}

fn remove_recursive(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 解压
// ---------------------------------------------------------------------------

/// 解压 `root` 下所有 .zip，各自解到同名子目录里。返回解压的个数。
///
/// 只处理 zip：rar 和 7z 需要外部工具，装没装不好说，不如不做。
fn extract_zips(root: &Path) -> Result<usize> {
    let mut count = 0;
    for archive in find_zips(root)? {
        let target = archive.with_extension("");
        let target = unique_path(
            target.parent().unwrap_or(root),
            target.file_name().context("压缩包没有文件名")?,
        );
        extract_one(&archive, &target)
            .with_context(|| format!("解压 {} 失败", archive.display()))?;
        count += 1;
    }
    Ok(count)
}

fn find_zips(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if root.is_file() {
        if is_zip(root) {
            out.push(root.to_path_buf());
        }
        return Ok(out);
    }
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            out.extend(find_zips(&path)?);
        } else if is_zip(&path) {
            out.push(path);
        }
    }
    Ok(out)
}

fn is_zip(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn extract_one(archive: &Path, target: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    std::fs::create_dir_all(target)?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // 压缩包里的路径是外部输入。不挡住 `../` 和绝对路径的话，
        // 一个构造过的 zip 能往目录外面写任意文件（zip slip）。
        let Some(rel) = safe_entry_path(entry.name()) else {
            tracing::warn!(entry = entry.name(), "跳过压缩包里的可疑路径");
            continue;
        };

        let out = target.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
    }
    Ok(())
}

/// 只接受纯相对路径；带 `..`、根目录、盘符前缀的一律拒绝。
fn safe_entry_path(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let out: PathBuf = path.components().collect();
    (!out.as_os_str().is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mydl-auto-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn rejects_zip_slip_paths() {
        // 这些都是能写到解压目录外面去的，必须拒绝。
        assert!(safe_entry_path("../evil.sh").is_none());
        assert!(safe_entry_path("a/../../evil.sh").is_none());
        assert!(safe_entry_path("/etc/passwd").is_none());
        assert!(safe_entry_path("").is_none());

        assert_eq!(safe_entry_path("a/b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(safe_entry_path("b.txt"), Some(PathBuf::from("b.txt")));
    }

    #[test]
    fn never_overwrites_existing() {
        let dir = tmp("unique");
        std::fs::write(dir.join("a.mkv"), b"first").unwrap();

        let p2 = unique_path(&dir, std::ffi::OsStr::new("a.mkv"));
        assert_eq!(p2.file_name().unwrap(), "a (2).mkv");

        std::fs::write(&p2, b"second").unwrap();
        let p3 = unique_path(&dir, std::ffi::OsStr::new("a.mkv"));
        assert_eq!(p3.file_name().unwrap(), "a (3).mkv");

        // 没扩展名的也要能处理。
        std::fs::create_dir(dir.join("folder")).unwrap();
        let f2 = unique_path(&dir, std::ffi::OsStr::new("folder"));
        assert_eq!(f2.file_name().unwrap(), "folder (2)");

        // 原文件不能被动过。
        assert_eq!(std::fs::read(dir.join("a.mkv")).unwrap(), b"first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moves_directory_and_keeps_content() {
        let base = tmp("move");
        let src = base.join("种子内容");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/a.txt"), b"hello").unwrap();
        let dest = base.join("媒体库");

        let moved = move_into(&src, &dest).unwrap();

        assert!(!src.exists(), "源目录该被移走");
        assert_eq!(moved, dest.join("种子内容"));
        assert_eq!(std::fs::read(moved.join("sub/a.txt")).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn move_does_not_clobber_existing_target() {
        let base = tmp("clobber");
        let src = base.join("片子");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("new.txt"), b"new").unwrap();

        let dest = base.join("媒体库");
        std::fs::create_dir_all(dest.join("片子")).unwrap();
        std::fs::write(dest.join("片子/old.txt"), b"old").unwrap();

        let moved = move_into(&src, &dest).unwrap();

        assert_eq!(moved, dest.join("片子 (2)"));
        // 已有内容必须原封不动。
        assert_eq!(std::fs::read(dest.join("片子/old.txt")).unwrap(), b"old");
        assert_eq!(std::fs::read(moved.join("new.txt")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod extract_tests {
    use super::*;
    use std::io::Write;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mydl-zip-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 造一个 zip，entries 是 (路径, 内容)。路径可以是恶意的。
    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn extracts_into_sibling_directory() {
        let dir = tmp("ok");
        make_zip(
            &dir.join("bundle.zip"),
            &[("readme.txt", b"hi"), ("sub/deep.txt", b"deep")],
        );

        let n = extract_zips(&dir).unwrap();

        assert_eq!(n, 1);
        assert_eq!(std::fs::read(dir.join("bundle/readme.txt")).unwrap(), b"hi");
        assert_eq!(std::fs::read(dir.join("bundle/sub/deep.txt")).unwrap(), b"deep");
        // 原压缩包保留。
        assert!(dir.join("bundle.zip").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真正跑一遍 zip slip 攻击：压缩包里带 ../ 路径，试图写到解压目录外面。
    #[test]
    fn zip_slip_cannot_escape_target_dir() {
        let base = tmp("slip");
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();

        make_zip(
            &work.join("evil.zip"),
            &[
                ("../../pwned.txt", b"escaped!"),
                ("safe.txt", b"fine"),
            ],
        );

        extract_zips(&work).unwrap();

        // 恶意条目被跳过，正常条目照常解出来。
        assert!(!base.join("pwned.txt").exists(), "文件逃出了解压目录");
        assert!(!base.parent().unwrap().join("pwned.txt").exists());
        assert_eq!(std::fs::read(work.join("evil/safe.txt")).unwrap(), b"fine");
        let _ = std::fs::remove_dir_all(&base);
    }
}
