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
    // 必须在创建托盘/菜单/窗口之前启用 DPI 感知，否则高分屏下系统会
    // 对原生 HMENU 做位图拉伸，托盘右键菜单中文字体会发糊。
    system::dpi::enable();

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

    // 程序启动后默认启动本地代理（sing-box 核心）。
    // 失败不阻断 WebUI / 托盘：用户仍可在面板里手动重试。
    if let Err(e) = auto_start_local_proxy(&state).await {
        eprintln!("[proxy] 启动时自动启动本地代理失败: {e}");
        state.log_with("core", "error", format!("启动时自动启动本地代理失败: {e}"));
    }

    // 若配置要求系统代理，则在核心起来后同步 WinINET。
    {
        let cfg = state.config.read().await;
        if cfg.system_proxy {
            match crate::system::sysproxy::enable(cfg.proxy_port) {
                Ok(_) => {
                    println!(
                        "[proxy] 系统代理已启用 → 127.0.0.1:{}",
                        cfg.proxy_port
                    );
                    state.log_with(
                        "config",
                        "info",
                        format!("启动时启用系统代理 → 127.0.0.1:{}", cfg.proxy_port),
                    );
                }
                Err(e) => {
                    eprintln!("[proxy] 启动时启用系统代理失败: {e}");
                    state.log_with("config", "error", format!("启动时启用系统代理失败: {e}"));
                }
            }
        }
    }

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

/// 程序启动时默认拉起 sing-box 本地代理。
/// 逻辑与 `/api/core/start` 对齐：用 visible_profile 渲染配置并更新 RuntimeStatus。
async fn auto_start_local_proxy(state: &Arc<AppState>) -> Result<()> {
    let p = state.visible_profile().await;
    let cfg = state.config.read().await.clone();
    let node_name = p
        .selected_node
        .as_ref()
        .and_then(|id| p.nodes.iter().find(|n| &n.id == id).map(|n| n.name.clone()));
    state.log_with(
        "core",
        "info",
        format!(
            "启动时自动启动本地代理: 模式={:?} 节点={} 节点数={}",
            p.mode,
            node_name.as_deref().unwrap_or("无"),
            p.nodes.len()
        ),
    );
    state.core.start(&p, &cfg).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    {
        let mut st = state.status.write().await;
        st.running = true;
        st.mode = p.mode;
        st.current_node = p.selected_node.clone();
    }
    state.emit(app::AppEvent::Status {
        running: true,
        mode: p.mode,
        current_node: p.selected_node.clone(),
    });
    state.log_with("core", "info", "本地代理已随程序启动");
    println!("[proxy] 本地代理已启动");
    Ok(())
}
