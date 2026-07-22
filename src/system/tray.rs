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
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
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
    ToggleSystemProxy,
    ToggleTun,
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
    // 初始状态：托盘线程不能 await，用 try_read 非阻塞取一次。
    // current_node 存的是节点 id，托盘顶部要显示 name。
    let (initial_running, initial_node_label, initial_autostart, initial_sysproxy, initial_tun) = {
        let st = state
            .status
            .try_read()
            .map(|g| (g.running, g.current_node.clone()))
            .unwrap_or((false, None));
        let label = resolve_node_label(&state, st.1.as_ref());
        let (au, sp, tun) = state
            .config
            .try_read()
            .map(|g| (g.autostart, g.system_proxy, g.enable_tun))
            .unwrap_or((false, false, false));
        (st.0, label, au, sp, tun)
    };

    // 按系统代理 / TUN 状态叠角标（右上紫 / 左上黄）
    let icon = make_icon(initial_sysproxy, initial_tun);

    let status_item = MenuItem::with_id(
        "status",
        status_text(initial_running, &initial_node_label),
        false,
        None,
    );
    let start_item = MenuItem::with_id("start", "启动代理", true, None);
    let stop_item = MenuItem::with_id("stop", "停止代理", true, None);
    let restart_item = MenuItem::with_id("restart", "重启代理", true, None);
    let update_item = MenuItem::with_id("update", "更新全部订阅", true, None);
    // muda CheckMenuItem::with_id 参数顺序是 (id, text, enabled, checked, accel)。
    // 之前误写成 (id, text, checked, enabled)，导致配置为 false 时菜单项被禁用、看起来勾着却点不动。
    let sysproxy_item =
        CheckMenuItem::with_id("sysproxy", "系统代理", true, initial_sysproxy, None);
    let tun_item = CheckMenuItem::with_id("tun", "TUN 模式", true, initial_tun, None);
    let autostart_item =
        CheckMenuItem::with_id("autostart", "开机启动", true, initial_autostart, None);
    let elevated = crate::system::admin::is_elevated();
    let elevate_item = MenuItem::with_id(
        "elevate",
        if elevated {
            "已以管理员身份运行"
        } else {
            "以管理员身份运行"
        },
        !elevated,
        None,
    );
    let open_item = MenuItem::with_id("open", "打开管理面板", true, None);
    let quit_item = MenuItem::with_id("quit", "退出", true, None);
    let separator = PredefinedMenuItem::separator();
    let separator2 = PredefinedMenuItem::separator();
    let separator3 = PredefinedMenuItem::separator();
    let separator4 = PredefinedMenuItem::separator();

    let menu = Menu::new();
    let _ = menu.append(&status_item);
    let _ = menu.append(&separator);
    let _ = menu.append(&start_item);
    let _ = menu.append(&stop_item);
    let _ = menu.append(&restart_item);
    let _ = menu.append(&update_item);
    let _ = menu.append(&separator2);
    let _ = menu.append(&sysproxy_item);
    let _ = menu.append(&tun_item);
    let _ = menu.append(&autostart_item);
    let _ = menu.append(&elevate_item);
    let _ = menu.append(&separator3);
    let _ = menu.append(&open_item);
    let _ = menu.append(&separator4);
    let _ = menu.append(&quit_item);

    let mut autostart_on = initial_autostart;
    let mut sysproxy_on = initial_sysproxy;
    let mut tun_on = initial_tun;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        // 左键不要弹菜单：左键只用来打开管理面板；菜单留给右键。
        // 否则会出现：左键点开菜单 → 再点桌面关闭菜单时，体验上像“误开了面板”
        // （实际是 Left+Up 已触发 OpenWebUI，与菜单弹出叠在一起）。
        .with_menu_on_left_click(false)
        .with_menu_on_right_click(true)
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
                "sysproxy" => {
                    sysproxy_on = !sysproxy_on;
                    sysproxy_item.set_checked(sysproxy_on);
                    let _ = cmd_tx.send(AppCommand::ToggleSystemProxy);
                }
                "tun" => {
                    tun_on = !tun_on;
                    tun_item.set_checked(tun_on);
                    let _ = cmd_tx.send(AppCommand::ToggleTun);
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

        // 托盘图标点击策略（对齐 Clash Verge / 常见代理客户端）：
        // - 左键松开：打开管理面板（且 builder 已 with_menu_on_left_click(false)，
        //   左键不会再弹菜单，避免“点图标出菜单 + 再点桌面关菜单却感觉误开面板”）。
        // - 右键：只弹菜单（builder with_menu_on_right_click(true)），不打开面板。
        // - Down / 中键 / DoubleClick / Enter / Move / Leave：忽略。
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            match ev {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    let _ = cmd_tx.send(AppCommand::OpenWebUI);
                }
                _ => {}
            }
        }

        // 状态变化 → 刷新 tooltip 与状态项文字；配置变化 → 刷新勾选
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AppEvent::Status {
                    running,
                    current_node,
                    ..
                } => {
                    // current_node 是 id，解析成 name 再显示
                    let label = resolve_node_label(&state, current_node.as_ref());
                    status_item.set_text(status_text(running, &label));
                    let _ = tray.set_tooltip(Some(tooltip_text(running)));
                }
                AppEvent::Config {
                    system_proxy,
                    enable_tun,
                } => {
                    sysproxy_on = system_proxy;
                    tun_on = enable_tun;
                    sysproxy_item.set_checked(sysproxy_on);
                    tun_item.set_checked(tun_on);
                    // 系统代理 / TUN 状态点：右上紫点、左上黄点
                    if let Err(e) = tray.set_icon(Some(make_icon(sysproxy_on, tun_on))) {
                        eprintln!("[proxy] 刷新托盘图标失败: {e}");
                    }
                }
                AppEvent::Profile { profile } => {
                    // 选中节点变化时，Profile 也会推送；刷新顶部名称
                    let label = profile
                        .selected_node
                        .as_ref()
                        .and_then(|id| {
                            profile
                                .nodes
                                .iter()
                                .find(|n| &n.id == id)
                                .map(|n| n.name.clone())
                        })
                        .or(profile.selected_node.clone());
                    // 保留当前 running 态（Profile 事件不带 running）
                    let running = state
                        .status
                        .try_read()
                        .map(|g| g.running)
                        .unwrap_or(false);
                    status_item.set_text(status_text(running, &label));
                }
                _ => {}
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
            AppCommand::ToggleSystemProxy => {
                let (new_on, port) = {
                    let mut c = state.config.write().await;
                    c.system_proxy = !c.system_proxy;
                    let v = c.system_proxy;
                    let port = c.proxy_port;
                    let _ = crate::config::manager::save_app_config(&state.data_dir, &c);
                    (v, port)
                };
                let r = if new_on {
                    crate::system::sysproxy::enable(port)
                } else {
                    crate::system::sysproxy::disable()
                };
                match r {
                    Ok(_) => {
                        let msg = if new_on {
                            format!("托盘：已启用系统代理 → 127.0.0.1:{port}")
                        } else {
                            "托盘：已关闭系统代理".into()
                        };
                        state.log_with("config", "info", msg);
                    }
                    Err(e) => {
                        // 回滚勾选
                        let mut c = state.config.write().await;
                        c.system_proxy = !new_on;
                        let _ = crate::config::manager::save_app_config(&state.data_dir, &c);
                        state.log_with("config", "error", format!("托盘切换系统代理失败: {e}"));
                    }
                }
                let cfg = state.config.read().await;
                state.emit(AppEvent::Config {
                    system_proxy: cfg.system_proxy,
                    enable_tun: cfg.enable_tun,
                });
            }
            AppCommand::ToggleTun => {
                let new_on = {
                    let mut c = state.config.write().await;
                    c.enable_tun = !c.enable_tun;
                    let v = c.enable_tun;
                    let _ = crate::config::manager::save_app_config(&state.data_dir, &c);
                    v
                };
                if new_on && !crate::system::admin::is_elevated() {
                    state.log(
                        "warn",
                        "托盘：已开启 TUN，但当前未以管理员身份运行，sing-box 可能无法创建虚拟网卡。",
                    );
                }
                // TUN 变更需要重建核心配置
                let (p, c) = (
                    state.visible_profile().await,
                    state.config.read().await.clone(),
                );
                if state.core.is_running().await {
                    let _ = state.core.reload_safe(&p, &c).await;
                } else {
                    let _ = state.core.write_config_only(&p, &c).await;
                }
                state.log_with(
                    "config",
                    "info",
                    if new_on {
                        "托盘：已启用 TUN 模式"
                    } else {
                        "托盘：已关闭 TUN 模式"
                    },
                );
                state.emit(AppEvent::Config {
                    system_proxy: c.system_proxy,
                    enable_tun: new_on,
                });
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
                // 退出前关闭系统代理，避免用户系统仍指向本进程端口。
                if state.config.read().await.system_proxy {
                    let _ = crate::system::sysproxy::disable();
                }
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

/// 把 status 里的节点 id 解析成 profile 中的显示名；找不到则回退 id。
fn resolve_node_label(state: &AppState, node_id: Option<&String>) -> Option<String> {
    let id = node_id?;
    if let Ok(p) = state.profile.try_read() {
        if let Some(n) = p.nodes.iter().find(|n| &n.id == id) {
            return Some(n.name.clone());
        }
    }
    Some(id.clone())
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
        "YAP-XFISH — {}",
        if running { "代理运行中" } else { "代理已停止" }
    )
}

/// 托盘图标：嵌入 X-FISH 像素图 32x32，并按状态叠角标。
/// - 系统代理启用：右上角亮紫色圆点
/// - TUN 启用：左上角亮黄色圆点
fn make_icon(system_proxy: bool, enable_tun: bool) -> Icon {
    const W: u32 = 32;
    const H: u32 = 32;
    let mut rgba = include_bytes!("../../assets/tray_32.rgba").to_vec();
    debug_assert_eq!(rgba.len(), (W * H * 4) as usize);

    // 亮紫 / 亮黄（高饱和，托盘小图上可辨）
    const PURPLE: [u8; 4] = [192, 38, 211, 255]; // #c026d3
    const YELLOW: [u8; 4] = [250, 204, 21, 255]; // #facc15
    const RING: [u8; 4] = [255, 255, 255, 230]; // 细白描边，提高对比

    if enable_tun {
        // 左上角
        paint_dot(&mut rgba, W, 5.0, 5.0, 4.2, YELLOW, RING);
    }
    if system_proxy {
        // 右上角
        paint_dot(&mut rgba, W, (W as f32) - 6.0, 5.0, 4.2, PURPLE, RING);
    }

    Icon::from_rgba(rgba, W, H).expect("生成托盘图标失败")
}

/// 在 32x32 RGBA 上画一个带描边的实心圆点（中心 cx,cy，半径 r）。
fn paint_dot(rgba: &mut [u8], width: u32, cx: f32, cy: f32, r: f32, fill: [u8; 4], ring: [u8; 4]) {
    let r_outer = r + 1.1;
    let r2 = r * r;
    let r_outer2 = r_outer * r_outer;
    let min_x = ((cx - r_outer).floor() as i32).max(0) as u32;
    let max_x = ((cx + r_outer).ceil() as i32).min(width as i32 - 1) as u32;
    let min_y = ((cy - r_outer).floor() as i32).max(0) as u32;
    let max_y = ((cy + r_outer).ceil() as i32).min(width as i32 - 1) as u32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d2 = dx * dx + dy * dy;
            if d2 > r_outer2 {
                continue;
            }
            let i = ((y * width + x) * 4) as usize;
            let color = if d2 <= r2 { fill } else { ring };
            rgba[i] = color[0];
            rgba[i + 1] = color[1];
            rgba[i + 2] = color[2];
            rgba[i + 3] = color[3];
        }
    }
}
