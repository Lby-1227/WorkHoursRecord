// Modify:原生Windows工时应用入口，不创建控制台窗口。$saving-ai-agent-tokens
#![windows_subsystem = "windows"]
mod native;
mod ui;
/// 功能：启动独立单实例界面；无输入，失败只显示固定提示，不输出凭证。
fn main() {
    if let Err(error) = ui::run() { native::message(&error.to_string()); }
}
