// 编译期动作：
// 1) webui/dist 变化时重新触发构建（rust-embed 嵌入前端）
// 2) Windows 下把 assets/app.ico 嵌入 exe 作为应用图标
// 3) 嵌入 DPI 感知清单，避免托盘菜单在高分屏位图拉伸发糊
fn main() {
    println!("cargo:rerun-if-changed=webui/dist");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/tray_32.rgba");
    println!("cargo:rerun-if-changed=assets/app.manifest");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app.ico");
        res.set("ProductName", "YAP-XFISH");
        res.set("FileDescription", "YAP-XFISH — Windows Sing-box 代理客户端");
        // 清单：Per-Monitor V2 DPI 感知 + longPathAware
        if std::path::Path::new("assets/app.manifest").exists() {
            res.set_manifest_file("assets/app.manifest");
        }
        if let Err(e) = res.compile() {
            println!("cargo:warning=winres 嵌入图标/清单失败（可忽略）: {e}");
        }
    }
}
