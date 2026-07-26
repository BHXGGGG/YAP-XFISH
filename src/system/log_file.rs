//! 默认日志文件：`data/logs/yap-xfish.log`
//!
//! - 应用层 `AppState::log_with` 与核心 stdout/stderr 都会追加写入。
//! - 简单按大小滚动：超过约 8 MiB 时把当前文件改名为 `.1`，再开新文件。
//! - 失败静默（不影响主流程）；无 data_dir 时 no-op。

use once_cell::sync::OnceCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const LOG_FILE: &str = "yap-xfish.log";
const MAX_BYTES: u64 = 8 * 1024 * 1024;

static LOG_PATH: OnceCell<PathBuf> = OnceCell::new();
static LOG_FILE_HANDLE: OnceCell<Mutex<Option<File>>> = OnceCell::new();

/// 启动时调用一次：创建 `data/logs/` 并打开追加写句柄。
pub fn init(data_dir: &Path) {
    let dir = data_dir.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(LOG_FILE);
    let _ = LOG_PATH.set(path.clone());
    let file = open_append(&path);
    let _ = LOG_FILE_HANDLE.set(Mutex::new(file));
}

fn open_append(path: &Path) -> Option<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// 追加一行：`YYYY-MM-DD HH:MM:SS [source/level] message`
pub fn append(source: &str, level: &str, message: &str) {
    let Some(path) = LOG_PATH.get() else { return };
    let Some(cell) = LOG_FILE_HANDLE.get() else { return };

    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("{ts} [{source}/{level}] {message}\n");

    let mut guard = match cell.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    // 按大小滚动
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= MAX_BYTES {
            let rotated = path.with_extension("log.1");
            let _ = std::fs::rename(path, &rotated);
            *guard = open_append(path);
        }
    }

    if guard.is_none() {
        *guard = open_append(path);
    }
    if let Some(f) = guard.as_mut() {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

/// 当前日志文件路径（若已 init）。
#[allow(dead_code)]
pub fn path() -> Option<PathBuf> {
    LOG_PATH.get().cloned()
}
