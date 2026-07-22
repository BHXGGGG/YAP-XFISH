// 编译期无需额外动作：rust-embed 通过 derive 宏在编译时嵌入 webui/dist。
// 仅声明在 webui/dist 变化时重新触发构建。
fn main() {
    println!("cargo:rerun-if-changed=webui/dist");
}
