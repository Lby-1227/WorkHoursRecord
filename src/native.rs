// Modify:仅操作Rust工时应用自己的窗口、图标和托盘；参考独立Win32消息线程。$saving-ai-agent-tokens
use std::{ptr, sync::mpsc, thread};
use anyhow::{ensure, Result};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender, UnboundedReceiver};
use windows_sys::Win32::{Foundation::*, Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::ReleaseCapture, Shell::*, WindowsAndMessaging::*}};

#[derive(Clone, Copy)]
pub enum TrayCommand { Restore, Main, Floating, Refresh, Quit }
pub struct Tray { pub events: UnboundedReceiver<TrayCommand>, handle: usize, worker: Option<thread::JoinHandle<()>> }
pub struct Instance(HANDLE);
pub struct Windows { pub main: usize, pub floating: usize }
const CALLBACK: u32 = WM_APP + 91;

/// 功能：生成Win32字符串；输入文本，输出末尾带零的UTF16。
fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }
/// 功能：显示本应用固定错误；输入不含凭证的提示，无输出。
pub fn message(text: &str) { unsafe { MessageBoxW(ptr::null_mut(), wide(text).as_ptr(), wide("工时记录").as_ptr(), MB_OK | MB_ICONINFORMATION); } }
impl Instance {
    /// 功能：限制Rust应用单实例；无输入，输出持有互斥句柄的对象，不影响WPF版。
    pub fn acquire() -> Result<Self> {
        let handle = unsafe { CreateMutexW(ptr::null(), 0, wide("Local\\WorkHoursRecord.RustGpui").as_ptr()) };
        ensure!(!handle.is_null(), "无法创建单实例锁。");
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS { unsafe { CloseHandle(handle); } anyhow::bail!("Rust工时记录已经运行，请从系统托盘打开。"); }
        Ok(Self(handle))
    }
}
impl Drop for Instance {
    /// 功能：释放本实例句柄；输入自身，无输出。
    fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
}
impl Windows {
    /// 功能：切换主窗口或卡片；输入目标模式及置顶状态，无输出，不改变后台计时。
    pub fn switch(&self, floating: bool, topmost: bool, position: Option<(f32,f32)>) {
        unsafe {
            let show = if floating { self.floating } else { self.main } as HWND;
            let hide = if floating { self.main } else { self.floating } as HWND;
            ShowWindow(hide, SW_HIDE); constrain(show, None);
            SetWindowPos(show, if floating && topmost { HWND_TOPMOST } else { HWND_NOTOPMOST },0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE);
            ShowWindow(show, SW_RESTORE);
            // Modify:Windows隐藏弹窗恢复时可能沿用前台主窗口的placement，显示后固定卡片的逻辑尺寸。
            if floating { place(show as usize,position); }
            SetForegroundWindow(show);
        }
    }
}
/// 功能：从资源设置程序自己的窗口图标；输入句柄，无输出。
pub fn set_icon(handle: usize) {
    unsafe {
        let module = GetModuleHandleW(ptr::null());
        for (kind, size) in [(ICON_SMALL, 16), (ICON_BIG, 32)] {
            let icon = LoadImageW(module, 1 as *const u16, IMAGE_ICON, size, size, LR_SHARED);
            if !icon.is_null() { SendMessageW(handle as HWND, WM_SETICON, kind as usize, icon as isize); }
        }
    }
}
/// 功能：取消透明弹窗系统边框；输入卡片句柄，无输出，内容圆角由GPUI绘制。
pub fn transparent_border(handle: usize) {
    unsafe { let none: u32 = 0xfffffffe; DwmSetWindowAttribute(handle as HWND, 34, &none as *const _ as _, 4); }
}
/// 功能：显示或隐藏自己的窗口；输入句柄与状态，无输出。
pub fn hide(handle: usize) { unsafe { ShowWindow(handle as HWND, SW_HIDE); } }
/// 功能：更新卡片置顶；输入卡片句柄及状态，无输出。
pub fn pin(handle: usize, enabled: bool) {
    unsafe { SetWindowPos(handle as HWND, if enabled {HWND_TOPMOST} else {HWND_NOTOPMOST},0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE); }
}
/// 功能：放置卡片到保存位置或工作区右侧；输入句柄与可选坐标，无输出，自动限制在可见工作区。
pub fn place(handle: usize, position: Option<(f32,f32)>) {
    unsafe {
        compact_size(handle as HWND);
        if let Some((x,y)) = position { SetWindowPos(handle as HWND, ptr::null_mut(),x as i32,y as i32,0,0,SWP_NOSIZE|SWP_NOZORDER|SWP_NOACTIVATE); }
        constrain(handle as HWND, if position.is_none() {Some(true)} else {None});
    }
}
/// 功能：保证悬浮窗口为368×224逻辑像素；输入本应用卡片句柄，无输出，按窗口DPI换算后更新恢复尺寸。
unsafe fn compact_size(handle:HWND) {
    unsafe {
        let scale=GetDpiForWindow(handle).max(96) as f32/96.;
        // Modify:368×224指内容区，需加回Windows非客户区边距，否则DPI缩放下卡片文字会被挤压。
        // let width=(368.*scale).round() as i32;let height=(224.*scale).round() as i32;
        let mut outer:RECT=std::mem::zeroed();let mut client:RECT=std::mem::zeroed();
        GetWindowRect(handle,&mut outer);GetClientRect(handle,&mut client);
        let width=(368.*scale).round() as i32+((outer.right-outer.left)-(client.right-client.left)).max(0);
        let height=(224.*scale).round() as i32+((outer.bottom-outer.top)-(client.bottom-client.top)).max(0);
        SetWindowPos(handle,ptr::null_mut(),0,0,width,height,SWP_NOMOVE|SWP_NOZORDER|SWP_NOACTIVATE);
        let mut placement:WINDOWPLACEMENT=std::mem::zeroed();placement.length=std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(handle,&mut placement)!=0 {
            placement.rcNormalPosition.right=placement.rcNormalPosition.left+width;
            placement.rcNormalPosition.bottom=placement.rcNormalPosition.top+height;
            SetWindowPlacement(handle,&placement);
        }
    }
}
/// 功能：校正本窗口位置；输入句柄及是否靠右初始化，无输出，使用该窗口所在屏幕工作区。
unsafe fn constrain(handle: HWND, first: Option<bool>) {
    unsafe {
        let monitor = MonitorFromWindow(handle, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed(); info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mut rect: RECT = std::mem::zeroed();
        if GetMonitorInfoW(monitor, &mut info) == 0 || GetWindowRect(handle, &mut rect) == 0 { return; }
        let area = info.rcWork; let width = rect.right-rect.left; let height = rect.bottom-rect.top;
        let scale = GetDpiForWindow(handle).max(96) as f32 / 96.;
        let x = if first.is_some() { area.right-width-(24.*scale) as i32 } else { rect.left };
        let y = if first.is_some() { area.top+(40.*scale) as i32 } else { rect.top };
        SetWindowPos(handle,ptr::null_mut(),x.clamp(area.left,(area.right-width).max(area.left)),y.clamp(area.top,(area.bottom-height).max(area.top)),0,0,SWP_NOSIZE|SWP_NOZORDER|SWP_NOACTIVATE);
    }
}
/// 功能：拖动本应用卡片标题区域；输入句柄，输出拖动后的物理坐标供位置记忆。
pub fn drag(handle: usize) -> Option<(f32,f32)> {
    unsafe {
        ReleaseCapture(); SendMessageW(handle as HWND, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0); constrain(handle as HWND, None);
        let mut rect: RECT = std::mem::zeroed(); if GetWindowRect(handle as HWND, &mut rect) != 0 { Some((rect.left as f32,rect.top as f32)) } else { None }
    }
}
/// 功能：处理本应用托盘消息；输入Win32回调，输出Win32返回值，通过通道发送窗口操作。
unsafe extern "system" fn procedure(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE { let create = &*(l as *const CREATESTRUCTW); SetWindowLongPtrW(window,GWLP_USERDATA,create.lpCreateParams as isize); }
        let sender = GetWindowLongPtrW(window,GWLP_USERDATA) as *const UnboundedSender<TrayCommand>;
        if sender.is_null() { return DefWindowProcW(window,message,w,l); }
        match message {
            CALLBACK => {
                if l as u32 == WM_LBUTTONUP { let _ = (*sender).send(TrayCommand::Restore); }
                if l as u32 == WM_RBUTTONUP {
                    let menu = CreatePopupMenu();
                    for (id,label) in [(1,"打开主界面"),(2,"悬浮模式"),(3,"刷新当期"),(4,"退出")] { AppendMenuW(menu,MF_STRING,id,wide(label).as_ptr()); }
                    let mut point = std::mem::zeroed(); GetCursorPos(&mut point); SetForegroundWindow(window);
                    let selected = TrackPopupMenu(menu,TPM_RETURNCMD|TPM_RIGHTBUTTON,point.x,point.y,0,window,ptr::null()); DestroyMenu(menu);
                    let command = match selected {1=>Some(TrayCommand::Main),2=>Some(TrayCommand::Floating),3=>Some(TrayCommand::Refresh),4=>Some(TrayCommand::Quit),_=>None};
                    if let Some(command) = command { let _=(*sender).send(command); }
                } 0
            },
            WM_QUERYENDSESSION => { let _=(*sender).send(TrayCommand::Quit); 1 },
            WM_CLOSE => { DestroyWindow(window); 0 },
            WM_DESTROY => { PostQuitMessage(0); 0 },
            _ => DefWindowProcW(window,message,w,l),
        }
    }
}
impl Tray {
    /// 功能：创建唯一图标和阻塞消息线程；无输入，输出事件接收器与生命周期句柄。
    pub fn new() -> Result<Self> {
        let (sender,events)=unbounded_channel(); let (ready,started)=mpsc::channel();
        let worker=thread::spawn(move || unsafe {
            let module=GetModuleHandleW(ptr::null()); let class=wide(&format!("WorkHoursRecordTray{}",std::process::id()));
            let mut wc:WNDCLASSW=std::mem::zeroed(); wc.hInstance=module;wc.lpfnWndProc=Some(procedure);wc.lpszClassName=class.as_ptr();
            if RegisterClassW(&wc)==0 { let _=ready.send(Err("无法注册托盘消息窗口"));return; }
            let sender=Box::new(sender);let window=CreateWindowExW(0,class.as_ptr(),wide("工时记录托盘").as_ptr(),0,0,0,0,0,ptr::null_mut(),ptr::null_mut(),module,&*sender as *const _ as _);
            if window.is_null(){let _=ready.send(Err("无法创建托盘消息窗口"));return;}
            let mut icon:NOTIFYICONDATAW=std::mem::zeroed();icon.cbSize=std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            icon.hWnd=window;icon.uID=1;icon.uFlags=NIF_MESSAGE|NIF_ICON|NIF_TIP;icon.uCallbackMessage=CALLBACK;
            icon.hIcon=LoadImageW(module,1 as *const u16,IMAGE_ICON,GetSystemMetrics(SM_CXSMICON),GetSystemMetrics(SM_CYSMICON),LR_SHARED) as HICON;
            for (slot,value) in icon.szTip.iter_mut().take(127).zip("工时记录 · Rust".encode_utf16()){*slot=value;}
            if Shell_NotifyIconW(NIM_ADD,&icon)==0 {let _=ready.send(Err("系统托盘创建失败"));DestroyWindow(window);return;}
            let _=ready.send(Ok(window as usize));let mut message:MSG=std::mem::zeroed();
            while GetMessageW(&mut message,ptr::null_mut(),0,0)>0 {TranslateMessage(&message);DispatchMessageW(&message);}
            Shell_NotifyIconW(NIM_DELETE,&icon);drop(sender);
        });
        let handle=started.recv_timeout(std::time::Duration::from_secs(5))?.map_err(|message|anyhow::anyhow!(message))?;
        Ok(Self{events,handle,worker:Some(worker)})
    }
}
impl Drop for Tray {
    /// 功能：移除托盘并结束消息线程；输入自身，无输出，不停止其他程序。
    fn drop(&mut self){unsafe{PostMessageW(self.handle as HWND,WM_CLOSE,0,0);}if let Some(worker)=self.worker.take(){let _=worker.join();}}
}
