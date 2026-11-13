// Modify:嵌入程序Logo及Windows版本资源；只生成构建产物。$saving-ai-agent-tokens
/// 功能：编译Windows资源；无输入，输出链接所需的图标及版本资源。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/app.rc", embed_resource::NONE).manifest_required().unwrap();
    }
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
}

