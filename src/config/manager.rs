use crate::app::AppConfig;
use crate::config::model::AppProfile;
use crate::error::AppResult;
use std::path::Path;

const APP_CONFIG_FILE: &str = "app_config.json";
const PROFILE_FILE: &str = "profile.json";

pub fn load_app_config(data_dir: &Path) -> AppResult<AppConfig> {
    let path = data_dir.join(APP_CONFIG_FILE);
    if path.exists() {
        let s = std::fs::read_to_string(&path)?;
        let cfg: AppConfig = serde_json::from_str(&s)?;
        Ok(cfg)
    } else {
        let cfg = AppConfig::default();
        save_app_config(data_dir, &cfg)?;
        Ok(cfg)
    }
}

pub fn save_app_config(data_dir: &Path, cfg: &AppConfig) -> AppResult<()> {
    let path = data_dir.join(APP_CONFIG_FILE);
    let s = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, s)?;
    Ok(())
}

pub fn load_profile(data_dir: &Path) -> AppResult<AppProfile> {
    let path = data_dir.join(PROFILE_FILE);
    if path.exists() {
        let s = std::fs::read_to_string(&path)?;
        let mut p: AppProfile = serde_json::from_str(&s)?;
        // 节点集合为空时强制清空选中状态，避免下次启动从空列表里回退到残留节点 ID。
        if p.nodes.is_empty() {
            p.selected_node = None;
        }
        Ok(p)
    } else {
        let p = AppProfile::default();
        save_profile(data_dir, &p)?;
        Ok(p)
    }
}

pub fn save_profile(data_dir: &Path, p: &AppProfile) -> AppResult<()> {
    let path = data_dir.join(PROFILE_FILE);
    let s = serde_json::to_string_pretty(p)?;
    std::fs::write(&path, s)?;
    Ok(())
}

/// 备份当前 sing-box 配置文件，返回备份路径（供订阅更新失败回滚使用）。
///
/// 同时在同目录维护滚动备份：仅保留最近 3 个 `config.backup.<ts>.json`，
/// 超出时按时间戳（文件名）升序删除最旧的。
pub fn backup_config(config_path: &Path) -> AppResult<Option<std::path::PathBuf>> {
    const MAX_BACKUPS: usize = 3;
    const BACKUP_PREFIX: &str = "config.backup.";
    const BACKUP_SUFFIX: &str = ".json";

    if !config_path.exists() {
        return Ok(None);
    }
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup = config_path
        .with_file_name(format!("config.backup.{}.json", ts));
    std::fs::copy(config_path, &backup)?;

    if let Some(parent) = config_path.parent() {
        let mut backups: Vec<std::path::PathBuf> = std::fs::read_dir(parent)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(BACKUP_PREFIX) && n.ends_with(BACKUP_SUFFIX))
                    .unwrap_or(false)
            })
            .collect();
        // 时间戳格式 %Y%m%d_%H%M%S 的字典序与时间顺序一致；按名字降序，最新的在前。
        backups.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for old in backups.into_iter().skip(MAX_BACKUPS) {
            let _ = std::fs::remove_file(&old);
        }
    }

    Ok(Some(backup))
}
