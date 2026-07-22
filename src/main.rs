// 发布版（release）以 Windows GUI 子系统编译：双击启动不再弹出黑色控制台窗口。
// debug 构建仍保留控制台，方便开发期排错。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod core;
mod error;
mod latency;
mod profile;
mod rules;
mod server;
mod subscription;
mod system;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use app::AppState;
use tokio::sync::mpsc::unbounded_channel;

#[tokio::main]
async fn main() -> Result<()> {
    // 单实例：已有实例在运行则直接退出（不抢占端口、不重复后台）。
    if !system::single_instance::ensure_single_instance() {
        eprintln!("[proxy] 已有实例在运行，本进程退出。");
        std::process::exit(0);
    }

    let data_dir = app::default_data_dir();
    std::fs::create_dir_all(&data_dir)?;
    // 首次使用便携 data 目录时，若旧版 %LOCALAPPDATA%\Proxy 存在则一次性复制过来，避免订阅/配置丢失。
    app::maybe_migrate_legacy(&data_dir);

    let (app_config, profile) = app::load_or_init(&data_dir)?;
    let state = Arc::new(AppState::new(app_config, profile, data_dir.clone()));

    // 按配置应用开机启动（HKCU Run）。
    system::autostart::apply_autostart(state.config.read().await.autostart);

    // 若启用 TUN 但未以管理员身份运行，sing-box 无法创建虚拟网卡，提前告警。
    if state.config.read().await.enable_tun && !crate::system::admin::is_elevated() {
        eprintln!("[proxy] 警告：已启用 TUN，但当前未以管理员身份运行；sing-box 将无法创建虚拟网卡。");
        state.log("warn", "已启用 TUN，但当前未以管理员身份运行。请通过托盘菜单「以管理员身份运行」重新启动以启用 TUN。");
    }

    println!("[proxy] 数据目录: {}", data_dir.display());
    println!(
        "[proxy] 管理面板: http://127.0.0.1:{}",
        state.config.read().await.web_port
    );

    // 启动订阅定时调度（周期 / 自定义 Cron 自动更新）
    subscription::scheduler::start(state.clone());

    // 启动系统托盘（独立线程 + Win32 消息泵）与托盘命令处理循环。
    let thread_id: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(None));
    let (cmd_tx, cmd_rx) = unbounded_channel();
    system::tray::start_tray(state.clone(), cmd_tx, thread_id.clone());
    tokio::spawn(system::tray::command_loop(
        state.clone(),
        cmd_rx,
        thread_id.clone(),
    ));

    // 运行 HTTP 服务（浏览器/托盘关闭不影响后台）。ctrl_c 退出。
    tokio::select! {
        r = server::run(state) => {
            if let Err(e) = r {
                eprintln!("[proxy] 服务错误: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("[proxy] 收到退出信号，停止后台…");
        }
    }
    Ok(())
}
