// 编译期动作：
// 1) webui/dist 变化时重新触发构建（rust-embed 嵌入前端）
// 2) Windows 下把 assets/app.ico 嵌入 exe 作为应用图标
// 3) 嵌入 DPI 感知清单，避免托盘菜单在高分屏位图拉伸发糊
fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
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
        if let Err(e) = compile_windows_resources(&res) {
            println!("cargo:warning=winres 嵌入图标/清单失败（可忽略）: {e}");
        }
    }
}

#[cfg(windows)]
fn compile_windows_resources(res: &winres::WindowsResource) -> std::io::Result<()> {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        return res.compile();
    }

    // LLVM-MinGW 的链接器不会从静态库中提取无外部符号的资源对象，需直接链接。
    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let rc = out_dir.join("resource.rc");
    let object = out_dir.join("resource.o");
    res.write_resource_file(&rc)?;
    let status = std::process::Command::new("windres")
        .arg("-I")
        .arg(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .args(["-O", "coff", "--codepage=65001"])
        .arg(&rc)
        .arg(&object)
        .status()?;
    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Could not compile Windows resources",
        ));
    }
    println!("cargo:rustc-link-arg={}", object.display());
    Ok(())
}
