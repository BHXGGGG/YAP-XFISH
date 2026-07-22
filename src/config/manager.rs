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
        let p: AppProfile = serde_json::from_str(&s)?;
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
pub fn backup_config(config_path: &Path) -> AppResult<Option<std::path::PathBuf>> {
    if !config_path.exists() {
        return Ok(None);
    }
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup = config_path
        .with_file_name(format!("config.backup.{}.json", ts));
    std::fs::copy(config_path, &backup)?;
    Ok(Some(backup))
}
