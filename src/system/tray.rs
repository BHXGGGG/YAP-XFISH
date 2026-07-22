//! 系统托盘：图标 + 右键菜单（当前节点 / 启停 / 更新 / 开机启动 / 打开面板 / 退出）+ Win32 消息泵。
//!
//! 重要约束（来自 tray-icon 文档）：
//! Windows 上必须在「创建托盘的同一线程」运行 Win32 消息泵，菜单点击才会触发。
//! 因此托盘在一个独立 OS 线程中创建并运行消息泵；菜单点击通过 `MenuEvent` 全局
//! channel 取出后派发为 `AppCommand`，交给 tokio 运行时里的 `command_loop` 执行
//! （CoreManager 的方法都是 async 的）。
//!
//! 注意：muda 的 `MenuItem` / `CheckMenuItem` 内部是 `Rc`（非 Send），不能跨线程共享。
//! 因此所有视觉更新（tooltip、状态文字、勾选态）都在托盘线程内完成 —— 通过订阅
//! `AppState` 的 broadcast 事件，在消息泵循环里刷新。
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use tray_icon::Icon;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::TrayIconBuilder;
use tray_icon::TrayIconEvent;
use windows_sys::Win32::Foundation::BOOL;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, SW_SHOWNORMAL, TranslateMessage,
};

use crate::app::{AppEvent, AppState};

/// 自定义退出消息，用于从其它线程唤醒托盘消息泵并干净退出。
const WM_USER_QUIT: u32 = 0x8000 + 1;

/// 托盘菜单派发的命令，由 tokio 运行时中的 `command_loop` 执行。
#[derive(Debug, Clone)]
pub enum AppCommand {
    Start,
    Stop,
    Restart,
    UpdateAll,
    OpenWebUI,
    ToggleAutostart,
    Elevate,
    Quit,
}

/// 启动托盘（独立 OS 线程）。线程内创建图标/菜单并进入消息泵。
/// 即便托盘初始化失败也仅打印错误，不影响后台与 WebUI。
pub fn start_tray(
    state: Arc<AppState>,
    cmd_tx: UnboundedSender<AppCommand>,
    thread_id: Arc<Mutex<Option<u32>>>,
) {
    std::thread::spawn(move || {
        if let Err(e) = run_tray(state, cmd_tx, thread_id) {
            eprintln!("[proxy] 托盘初始化失败（不影响后台）: {e}");
        }
    });
}

fn run_tray(
    state: Arc<AppState>,
    cmd_tx: UnboundedSender<AppCommand>,
    thread_id: Arc<Mutex<Option<u32>>>,
) -> anyhow::Result<()> {
    let icon = make_icon();

    // 初始状态：托盘线程不能 await，用 try_read 非阻塞取一次。
    let (initial_running, initial_node, initial_autostart) = {
        let st = state
            .status
            .try_read()
            .map(|g| (g.running, g.current_node.clone()))
            .unwrap_or((false, None));
        let au = state.config.try_read().map(|g| g.autostart).unwrap_or(false);
        (st.0, st.1, au)
    };

    let status_item = MenuItem::with_id("status", status_text(initial_running, &initial_node), false, None);
    let start_item = MenuItem::with_id("start", "启动代理", true, None);
    let stop_item = MenuItem::with_id("stop", "停止代理", true, None);
    let restart_item = MenuItem::with_id("restart", "重启代理", true, None);
    let update_item = MenuItem::with_id("update", "更新全部订阅", true, None);
    let autostart_item = CheckMenuItem::with_id("autostart", "开机启动", initial_autostart, true, None);
    let elevated = crate::system::admin::is_elevated();
    let elevate_item = MenuItem::with_id(
        "elevate",
        if elevated { "已以管理员身份运行" } else { "以管理员身份运行" },
        !elevated,
        None,
    );
    let open_item = MenuItem::with_id("open", "打开管理面板", true, None);
    let quit_item = MenuItem::with_id("quit", "退出", true, None);
    let separator = PredefinedMenuItem::separator();

    let menu = Menu::new();
    let _ = menu.append(&status_item);
    let _ = menu.append(&separator);
    let _ = menu.append(&start_item);
    let _ = menu.append(&stop_item);
    let _ = menu.append(&restart_item);
    let _ = menu.append(&update_item);
    let _ = menu.append(&separator);
    let _ = menu.append(&autostart_item);
    let _ = menu.append(&elevate_item);
    let _ = menu.append(&open_item);
    let _ = menu.append(&separator);
    let _ = menu.append(&quit_item);

    let mut autostart_on = initial_autostart;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(tooltip_text(initial_running))
        .with_icon(icon)
        .build()?;

    // 记录托盘线程 ID，供 Quit 时 PostThreadMessageW 唤醒消息泵退出。
    if let Ok(mut g) = thread_id.lock() {
        *g = Some(unsafe { GetCurrentThreadId() });
    }

    // 订阅状态事件，用于在托盘线程内刷新视觉。
    let mut rx = state.event_tx.subscribe();

    // Win32 消息泵：菜单点击与托盘点击都经此循环分发。
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        let ret: BOOL = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
        if ret == 0 {
            break; // 收到 WM_QUIT
        }
        if ret == -1 {
            continue;
        }
        if msg.message == WM_USER_QUIT {
            break;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // 菜单点击 → 派发命令
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id().as_ref().to_string();
            match id.as_str() {
                "start" => {
                    let _ = cmd_tx.send(AppCommand::Start);
                }
                "stop" => {
                    let _ = cmd_tx.send(AppCommand::Stop);
                }
                "restart" => {
                    let _ = cmd_tx.send(AppCommand::Restart);
                }
                "update" => {
                    let _ = cmd_tx.send(AppCommand::UpdateAll);
                }
                "open" => {
                    let _ = cmd_tx.send(AppCommand::OpenWebUI);
                }
                "autostart" => {
                    // 乐观更新勾选态（真实持久化在 command_loop 完成）
                    autostart_on = !autostart_on;
                    autostart_item.set_checked(autostart_on);
                    let _ = cmd_tx.send(AppCommand::ToggleAutostart);
                }
                "elevate" => {
                    let _ = cmd_tx.send(AppCommand::Elevate);
                }
                "quit" => {
                    let _ = cmd_tx.send(AppCommand::Quit);
                }
                _ => {}
            }
        }

        // 托盘图标点击（左键）→ 打开管理面板
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { .. } = ev {
                let _ = cmd_tx.send(AppCommand::OpenWebUI);
            }
        }

        // 状态变化 → 刷新 tooltip 与状态项文字
        while let Ok(ev) = rx.try_recv() {
            if let AppEvent::Status {
                running,
                current_node,
                ..
            } = ev
            {
                status_item.set_text(status_text(running, &current_node));
                let _ = tray.set_tooltip(Some(tooltip_text(running)));
            }
        }
    }

    // 干净退出：drop 托盘图标（引用计数归零会移除通知区图标），再结束进程。
    drop(tray);
    std::process::exit(0);
}

/// 在 tokio 运行时中消费托盘命令，操作异步的 CoreManager。
pub async fn command_loop(
    state: Arc<AppState>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AppCommand>,
    thread_id: Arc<Mutex<Option<u32>>>,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            AppCommand::Start => {
                let (p, c) = (
                    state.visible_profile().await,
                    state.config.read().await.clone(),
                );
                if state.core.start(&p, &c).await.is_ok() {
                    apply_status(&state, true).await;
                }
            }
            AppCommand::Stop => {
                let _ = state.core.stop().await;
                apply_status(&state, false).await;
            }
            AppCommand::Restart => {
                let (p, c) = (
                    state.visible_profile().await,
                    state.config.read().await.clone(),
                );
                if state.core.restart(&p, &c).await.is_ok() {
                    apply_status(&state, true).await;
                }
            }
            AppCommand::UpdateAll => {
                let _ = crate::subscription::manager::update_all_enabled(&state).await;
            }
            AppCommand::OpenWebUI => {
                open_webui(&state).await;
            }
            AppCommand::ToggleAutostart => {
                let new_on = {
                    let mut c = state.config.write().await;
                    c.autostart = !c.autostart;
                    let v = c.autostart;
                    let _ = crate::config::manager::save_app_config(&state.data_dir, &c);
                    v
                };
                let _ = crate::system::autostart::set_autostart(new_on);
            }
            AppCommand::Elevate => {
                if !crate::system::admin::is_elevated() {
                    if crate::system::admin::elevate_and_restart() {
                        // 提权实例已启动，退出当前非提权实例（让出互斥锁已在内部完成）。
                        let _ = state.core.stop().await;
                        std::process::exit(0);
                    } else {
                        state.log("warn", "提权失败或被取消，无法启用需要管理员权限的功能（如 TUN）。");
                    }
                }
            }
            AppCommand::Quit => {
                let _ = state.core.stop().await;
                let tid = *thread_id.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(tid) = tid {
                    unsafe {
                        let _ = PostThreadMessageW(tid, WM_USER_QUIT, 0, 0);
                    }
                } else {
                    std::process::exit(0);
                }
            }
        }
    }
}

async fn apply_status(state: &AppState, running: bool) {
    {
        let mut st = state.status.write().await;
        st.running = running;
        if running {
            let p = state.profile.read().await;
            st.mode = p.mode.clone();
            st.current_node = p.selected_node.clone();
        }
    }
    let st = state.status.read().await;
    state.emit(AppEvent::Status {
        running: st.running,
        mode: st.mode.clone(),
        current_node: st.current_node.clone(),
    });
}

/// 防止短时间内重复打开多个浏览器窗口：托盘图标点击、菜单「打开管理面板」、
/// 或事件偶发重复触发时，2 秒内只真正打开一次。
static LAST_OPEN: Mutex<Option<Instant>> = Mutex::new(None);

async fn open_webui(state: &AppState) {
    // 去抖：2 秒内的重复请求直接忽略，避免一次点击打开多个面板。
    {
        let mut g = LAST_OPEN.lock().unwrap();
        if let Some(last) = *g {
            if last.elapsed() < Duration::from_secs(2) {
                return;
            }
        }
        *g = Some(Instant::now());
    }

    let port = state.config.read().await.web_port;
    let url = format!("http://127.0.0.1:{}", port);
    #[cfg(target_os = "windows")]
    {
        shell_open_url(&url);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}

/// 用系统关联程序静默打开 URL（默认浏览器），不弹出黑色控制台窗口。
/// 等价于 `cmd /c start <url>`，但通过 ShellExecuteW 直接调用，无控制台闪烁。
#[cfg(target_os = "windows")]
fn shell_open_url(url: &str) {
    use std::os::windows::ffi::OsStrExt;

    let url_w: Vec<u16> = std::ffi::OsStr::new(url)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb_w: Vec<u16> = std::ffi::OsStr::new("open")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        ShellExecuteW(
            0,
            verb_w.as_ptr(),
            url_w.as_ptr(),
            std::ptr::null::<u16>(),
            std::ptr::null::<u16>(),
            SW_SHOWNORMAL,
        );
    }
}

fn status_text(running: bool, node: &Option<String>) -> String {
    if running {
        match node {
            Some(n) => format!("● 已连接 · {}", n),
            None => "● 已连接".to_string(),
        }
    } else {
        "○ 已停止".to_string()
    }
}

fn tooltip_text(running: bool) -> String {
    format!(
        "proxy-rs — {}",
        if running { "代理运行中" } else { "代理已停止" }
    )
}

/// 生成一个 32x32 的青色圆点图标（透明背景），无需外部图片资源。
fn make_icon() -> Icon {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let c = (size as f32) / 2.0;
    let r = 13.0f32;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            if (dx * dx + dy * dy).sqrt() <= r {
                rgba.extend_from_slice(&[45u8, 212u8, 191u8, 255u8]);
            } else {
                rgba.extend_from_slice(&[0u8, 0u8, 0u8, 0u8]);
            }
        }
    }
    Icon::from_rgba(rgba, size, size).expect("生成托盘图标失败")
}
