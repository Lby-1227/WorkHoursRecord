// Modify:三个GPUI Kit页面与共享状态的右侧悬浮模式；不使用WebView。$saving-ai-agent-tokens
use crate::native::{self, TrayCommand};
use gpui_kit::{component::{button::{Button,ButtonVariants}, slider::{Slider,SliderState,SliderEvent,SliderValue},
    table::{Table,TableHeader,TableBody,TableRow,TableHead,TableCell}, tooltip::Tooltip, ActiveTheme, Disableable, Sizable}, *};
use raw_window_handle::{HasWindowHandle,RawWindowHandle};
use work_hours_record::{attendance::open_url, domain::*, engine::{Engine,Snapshot,Command},store::{Store,MARKET}};
use std::borrow::Cow;
use chrono::Datelike;

struct Assets;
struct View {
    page: usize, floating: bool, snapshot: Snapshot, sender: tokio::sync::mpsc::UnboundedSender<Command>,
    windows: Option<native::Windows>, slider: Entity<SliderState>, subscriptions: Vec<Subscription>,
}
// Modify:悬浮窗口使用独立渲染实体，避免跨窗口复用主页面的GPUI布局缓存；数据和命令仍共享。
struct FloatingCard { shared: Entity<View>, _subscription: Subscription }
impl FloatingCard {
    /// 功能：创建独立卡片视图；输入共享状态和上下文，输出随数据更新重绘的卡片实体。
    fn new(shared:Entity<View>,cx:&mut Context<Self>)->Self {
        let subscription=cx.observe(&shared,|_,_,cx|cx.notify());
        Self{shared,_subscription:subscription}
    }
}
impl Render for FloatingCard {
    /// 功能：只绘制悬浮卡；输入窗口上下文，输出绑定共享工时状态的紧凑布局。
    fn render(&mut self,_:&mut Window,cx:&mut Context<Self>)->impl IntoElement {
        self.shared.update(cx,|view,cx|view.floating_card(cx))
    }
}
impl AssetSource for Assets {
    /// 功能：加载本应用Logo或Kit内置资源；输入资源路径，输出嵌入字节，不依赖外部图片文件。
    fn load(&self,path:&str)->gpui_kit::Result<Option<Cow<'static,[u8]>>> {
        if path=="app.png" { Ok(Some(Cow::Borrowed(include_bytes!("../assets/app.png")))) }
        else { gpui_kit::assets::Assets.load(path) }
    }
    /// 功能：列出Kit内置资源；输入目录，输出资源名集合。
    fn list(&self,path:&str)->gpui_kit::Result<Vec<SharedString>> { gpui_kit::assets::Assets.list(path) }
}

impl View {
    /// 功能：创建共享页面状态和透明度滑块；输入初始快照、命令通道及上下文，输出界面实体。
    fn new(snapshot:Snapshot,sender:tokio::sync::mpsc::UnboundedSender<Command>,window:&mut Window,cx:&mut Context<Self>)->Self {
        let slider=cx.new(|_|SliderState::new().min(20.).max(100.).step(1.).default_value(snapshot.settings.opacity*100.));
        let mut view=Self{page:if snapshot.needs_auth {2}else{0},floating:false,snapshot,sender,windows:None,slider,subscriptions:vec![]};
        // Modify:滑块变化仅预览共享背景，释放一次才发保存命令，不按每帧写入磁盘。
        let subscription=cx.subscribe_in(&view.slider,window,|this,_,event,_,cx|{
            match event {
                SliderEvent::Change(SliderValue::Single(value))=>{this.snapshot.settings.opacity=(*value/100.).clamp(0.2,1.);cx.notify();},
                SliderEvent::Release(SliderValue::Single(value))=>{let _=this.sender.send(Command::Opacity(*value/100.));},
                _=>{}
            }
        });
        view.subscriptions.push(subscription);view
    }
    /// 功能：发送后端操作；输入命令，无输出，通道不可用时给出固定提示。
    fn send(&mut self,command:Command) { if self.sender.send(command).is_err(){self.snapshot.detail="后台已退出，请重新打开工时记录。".into();} }
    /// 功能：切换窗口显示模式；输入悬浮标识，无输出，不影响计时和连接。
    fn switch(&mut self,floating:bool,cx:&mut Context<Self>) {
        self.floating=floating;if let Some(windows)=&self.windows {windows.switch(floating,self.snapshot.settings.topmost,self.snapshot.settings.floating_position);}cx.notify();
    }
    /// 功能：显示无数据时的占位工时；输入分钟及是否有数据，输出格式化文本。
    fn value(&self,minutes:i64)->String {if self.snapshot.summary.has_data {duration_text(minutes)}else{"—".into()}}
    /// 功能：复用三处以上的指标卡；输入标签、值及说明，输出可自适应的指标元素。
    fn metric(&self,label:&str,value:String,note:String,cx:&App)->AnyElement {
        div().flex_1().min_w(px(180.)).p_5().rounded_lg().border_1().border_color(cx.theme().border).bg(cx.theme().background)
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(label.to_string()))
            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).mt_2().child(value))
            .child(div().text_xs().mt_2().text_color(cx.theme().muted_foreground).child(note)).into_any_element()
    }
    /// 功能：绘制主界面；输入上下文，输出今日、周期汇总与两条宝箱任务。
    fn overview(&self,cx:&mut Context<Self>)->AnyElement {
        let state=&self.snapshot;let today=state.today();let summary=&state.summary;
        let today_value=today.and_then(|d|d.minutes).map(duration_text).unwrap_or_else(||"—".into());
        let today_note=today.map(|day|format!("{} · 上班 {}",state_text(day.state),punch_text(day.on))).unwrap_or_else(||"等待真实打卡数据".into());
        let mut content=div().flex().flex_col().gap_5()
            .child(div().flex().flex_wrap().gap_4()
                .child(self.metric("今日工时",today_value,today_note,cx))
                .child(self.metric("当期总净工时 · 含周末",self.value(summary.total),if summary.has_data{exact(summary.total)}else{"等待真实打卡数据".into()},cx)))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("当期工时明细"))
            .child(div().flex().flex_wrap().gap_4()
                .child(self.metric("工作日工时",self.value(summary.weekday),if summary.has_data{exact(summary.weekday)}else{"—".into()},cx))
                .child(self.metric("工作日累计加班",self.value(summary.overtime),"每天扣休后超过8小时的部分相加".into(),cx))
                .child(self.metric("周末净工时",self.value(summary.weekend),"沿用周末6/10小时扣休规则".into(),cx)))
            .child(Button::new("daily-entry").ghost().label("查看每日打卡统计 →").on_click(cx.listener(|this,_,_,cx|{this.page=1;cx.notify();})))
            .child(div().flex().flex_wrap().gap_4().child(self.treasure(Category::Total,cx)).child(self.treasure(Category::Weekday,cx)));
        if !state.fresh || !summary.reliable {content=content.child(div().text_sm().text_color(rgb(0x987127)).child(state.detail.clone()));}
        content.into_any_element()
    }
    /// 功能：绘制当前未领取宝箱；输入类别及上下文，输出一条进度、领取状态和每日预测。
    fn treasure(&self,category:Category,cx:&mut Context<Self>)->AnyElement {
        let state=&self.snapshot;let target=visible(&state.treasures,category).unwrap();
        let color=if category==Category::Total {rgb(0xc38b26)}else{rgb(0x3276e8)};
        let name=if category==Category::Total {"总净工时宝箱 · 含周末"}else{"工作日加班宝箱"};
        let target_text=if category==Category::Total {format!("> {} 小时",(target.required-1)/60)}else{format!("≥ {} 小时",target.required/60)};
        let remaining=(target.required-target.current).max(0);
        let progress=if state.summary.has_data {(target.current as f32/target.required as f32).clamp(0.,1.)}else{0.};
        let claim_text=if target.claimed {if target.reached{"本地已领取"}else{"已领 · 待核对"}}else if !state.summary.has_data {"等待数据"}else if !target.reached {"未达标"}else if target.can_claim {"领取宝箱"}else{"数据待核对"};
        let level=target.level;
        div().flex_1().min_w(px(290.)).p_5().rounded_lg().border_1().border_color(cx.theme().border).bg(cx.theme().background).flex().flex_col().gap_3()
            .child(div().flex().justify_between().child(div().font_weight(FontWeight::SEMIBOLD).child(name)).child(div().text_color(color).child(format!("¥{}",target.award))))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!("第{}档 · {target_text}",target.level)))
            .child(div().h(px(7.)).w_full().rounded_full().bg(cx.theme().secondary).child(div().h_full().w(relative(progress)).rounded_full().bg(color)))
            .child(div().text_sm().child(if state.summary.has_data {if remaining==0{"目标已达到".into()}else{format!("还差 {}",duration_text(remaining))}}else{"暂无可计入数据".into()}))
            .child(div().text_xs().text_color(color).child(state.plan(category)))
            .child(Button::new(("claim",if category==Category::Total {0usize}else{1usize})).label(claim_text).disabled(!target.can_claim||state.busy)
                .on_click(cx.listener(move|this,_,_,cx|{this.send(Command::Claim(category,level));cx.notify();})))
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child("达标后本地领取，领取后显示下一档"))
            .into_any_element()
    }
    /// 功能：绘制每日统计；输入上下文，输出共享每日数据的折线图与精确打卡表。
    fn statistics(&self,cx:&mut Context<Self>)->AnyElement {
        let state=&self.snapshot;
        let mut body=TableBody::new();
        if state.cache.is_some() {
            for day in &state.summary.days {
                let values=[format!("{} 周{}",day.date.format("%m/%d"),["日","一","二","三","四","五","六"][day.date.weekday().num_days_from_sunday() as usize]),
                    punch_text(day.on),if day.state==DayState::InProgress {"计时中".into()}else{punch_text(day.off)},
                    day.minutes.map(|_|day.rest.to_string()).unwrap_or_else(||"—".into()),
                    day.minutes.map(|m|m.to_string()).unwrap_or_else(||"—".into()),
                    day.minutes.map(|m|format!("{:.4}",m as f64/60.)).unwrap_or_else(||"—".into()),
                    day.overtime.map(|m|m.to_string()).unwrap_or_else(||"—".into()),state_text(day.state).into(),day.note.clone()];
                let mut row=TableRow::new();
                for value in values {row=row.child(TableCell::new().child(div().text_xs().child(value)));}
                body=body.child(row);
            }
        }
        let mut header=TableRow::new();for label in ["日期","上班","下班","扣休/分","净分钟","小时","加班/分","状态","说明"] {header=header.child(TableHead::new().child(label));}
        div().flex().flex_col().gap_4()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("每日净工时趋势"))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("单位：小时 · 今日计时中会更新 · 空缺表示无记录、待核对或未到日期"))
            .child(self.chart(cx))
            .child(div().text_lg().mt_3().font_weight(FontWeight::SEMIBOLD).child("每日打卡明细"))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(if state.summary.has_data {format!("当期合计：{}",exact(state.summary.total))}else{"等待真实打卡数据".into()}))
            .child(div().id("table-scroll").w_full().overflow_x_scroll().child(Table::new().small().child(TableHeader::new().child(header)).child(body)))
            .into_any_element()
    }
    /// 功能：按日历坐标绘制连续有效片段；输入上下文，输出不补零、不跨缺失日期连线的折线图。
    fn chart(&self,cx:&App)->AnyElement {
        let days=if self.snapshot.cache.is_some(){self.snapshot.summary.days.clone()}else{vec![]};
        let max_hours=(days.iter().filter_map(|day|day.minutes).max().unwrap_or(600) as f32/60./4.).ceil().max(2.)*4.;
        let paint_days=days.clone();let grid=cx.theme().border;let blue=rgb(0x3276e8);
        let mut plot=div().relative().w_full().h(px(205.)).rounded_lg().bg(cx.theme().background).border_1().border_color(cx.theme().border)
            .child(canvas(move|_,_,_|(),move|bounds,_,window,_|{
                let width=bounds.size.width.as_f32();let height=bounds.size.height.as_f32();
                let origin=bounds.origin;let left=40.;let top=15.;let usable_width=(width-left-15.).max(1.);let usable_height=(height-top-25.).max(1.);
                // Modify:每段重建path，缺失点结束片段；不将无记录转换成零或把间隔日期压缩。
                let mut path=PathBuilder::stroke(px(1.));
                for index in 0..=4 {let y=top+usable_height*index as f32/4.;path.move_to(origin+point(px(left),px(y)));path.line_to(origin+point(px(width-15.),px(y)));}
                if let Ok(path)=path.build(){window.paint_path(path,grid);}
                let mut line=PathBuilder::stroke(px(2.));let mut connected=false;
                for (index,day) in paint_days.iter().enumerate() {
                    if let Some(minutes)=day.minutes {
                        let x=left+usable_width*index as f32/(paint_days.len().saturating_sub(1)).max(1) as f32;
                        let y=top+usable_height*(1.-minutes as f32/60./max_hours);
                        let p=origin+point(px(x),px(y));if connected{line.line_to(p);}else{line.move_to(p);}connected=true;
                        window.paint_quad(quad(Bounds::new(p-point(px(3.),px(3.)),size(px(6.),px(6.))),px(3.),blue,px(0.),blue,BorderStyle::Solid));
                    }else{connected=false;}
                }
                if let Ok(line)=line.build(){window.paint_path(line,blue);}
            }).size_full());
        for index in 0..=4 {plot=plot.child(div().absolute().left(px(5.)).top(px(10.+165.*index as f32/4.)).text_xs().text_color(cx.theme().muted_foreground).child(format!("{:.0}",max_hours*(1.-index as f32/4.))));}
        // Modify:日期标签保留整周期，今日及所有精确秒/分钟细节同时可在下方表格核对。
        let mut labels=div().absolute().bottom(px(2.)).left(px(40.)).right(px(15.)).flex().justify_between().text_xs().text_color(cx.theme().muted_foreground);
        for (index,day) in days.iter().enumerate(){if index%5==0||index+1==days.len(){labels=labels.child(day.date.format("%m/%d").to_string());}}
        let mut points=div().absolute().left(px(40.)).right(px(15.)).top(px(15.)).bottom(px(25.));
        // Modify:只为有效点创建提示；今日计时中使用金色，提示给出分钟、小时及真实状态。
        for (index,day) in days.iter().enumerate(){if let Some(minutes)=day.minutes{
            let hint=format!("{} · {} · {}",day.date,exact(minutes),state_text(day.state));
            points=points.child(div().id(("chart-point",index)).absolute().left(relative(index as f32/days.len().saturating_sub(1).max(1) as f32))
                .top(relative(1.-minutes as f32/60./max_hours)).ml(px(-4.)).mt(px(-4.)).size(px(8.)).rounded_full()
                .bg(if day.state==DayState::InProgress{rgb(0xd59828)}else{rgb(0x3276e8)}).cursor_pointer()
                .tooltip(move|window,cx|Tooltip::new(hint.clone()).build(window,cx)));
        }}
        plot.child(points).child(labels).into_any_element()
    }
    /// 功能：绘制配置页面；输入上下文，输出官方连接、悬浮外观和只读规则说明。
    fn settings(&self,cx:&mut Context<Self>)->AnyElement {
        let state=&self.snapshot;let busy=state.busy;
        div().flex().flex_col().gap_5()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("钉钉考勤连接"))
            .child(div().text_color(cx.theme().muted_foreground).child("使用本人钉钉账号和正确组织授权，完成后自动读取当期历史与今日打卡。"))
            .child(div().flex().gap_3()
                .child(Button::new("connect").primary().label(if state.connection.connected_before{"重新连接钉钉"}else{"连接钉钉"}).disabled(busy).on_click(cx.listener(|this,_,_,cx|{this.send(Command::Connect);cx.notify();})))
                .child(Button::new("configuration").label("配置链接 ↗").on_click(cx.listener(|this,_,_,cx|{if open_url(MARKET).is_err(){this.snapshot.detail="无法打开官方配置网页。".into();}cx.notify();})))
                .child(Button::new("cancel").ghost().label("取消当前操作").disabled(!busy).on_click(cx.listener(|this,_,_,cx|{this.send(Command::Cancel);cx.notify();}))))
            .child(div().p_4().rounded_lg().bg(cx.theme().secondary).flex().flex_col().gap_2()
                .child(self.snapshot.status.clone()).child(div().text_sm().child(self.snapshot.detail.clone()))
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child("配置网页：启用“钉钉考勤”（ID 9668）→ 返回应用连接 → 浏览器授权。")))
            .child(div().text_lg().mt_2().font_weight(FontWeight::SEMIBOLD).child("悬浮卡外观"))
            .child(div().flex().justify_between().child("背景不透明度（仅悬浮卡）").child(format!("{:.0}%",state.settings.opacity*100.)))
            .child(Slider::new(&self.slider))
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child("范围20%—100%；文字、图标和按钮保持清晰，释放滑块后保存。"))
            .child(Button::new("pin-setting").label(if state.settings.topmost{"📌 悬浮置顶：已开启"}else{"📌 悬浮置顶：已关闭"}).on_click(cx.listener(|this,_,_,cx|{
                let enabled=!this.snapshot.settings.topmost;this.snapshot.settings.topmost=enabled;
                if let Some(windows)=&this.windows{native::pin(windows.floating,enabled);}this.send(Command::Pin(enabled));cx.notify();
            })))
            .child(div().text_lg().mt_2().font_weight(FontWeight::SEMIBOLD).child("当前工时规则"))
            .child(div().p_4().rounded_lg().bg(cx.theme().secondary).text_sm().flex().flex_col().gap_2()
                .child("北京时间 · 每月25日至次月24日，包含首尾。")
                .child("工作日只扣与12:00—13:00实际重合的午休；每天净工时超8小时的部分相加。")
                .child("周末跨度超过6小时扣1小时，超过10小时扣2小时，两档不叠加。")
                .child("总工时＞250/280/320小时：300/500/1000；工作日加班≥30/50/70小时：500/1000/1500。")
                .child("同类最高档，两类相加；宝箱仅记录本地领取，不关联公司奖金。")
                .child("总工时每日预测包含周末；工作日加班每日预测排除周六日。"))
            .into_any_element()
    }
    /// 功能：绘制紧凑悬浮卡；输入上下文，输出保持文字不透明的独立背景卡片。
    fn floating_card(&self,cx:&mut Context<Self>)->AnyElement {
        let opacity=self.snapshot.settings.opacity;let today=self.snapshot.today();let running=today.is_some_and(|d|d.state==DayState::InProgress);
        let title=if running{"今日工时 · 计时中"}else{"今日工时"};
        let today_text=today.and_then(|d|d.minutes).map(duration_text).unwrap_or_else(||"—".into());
        div().size_full().rounded_xl().overflow_hidden().font_family("Microsoft YaHei UI").text_sm().text_color(rgb(0x202d43)).flex().flex_col()
            .child(div().h(px(52.)).flex_shrink_0().bg(rgb(0x1e304d).alpha(opacity)).px_4().flex().items_center().justify_between()
                .child(div().flex().items_center().gap_2().cursor_move().on_mouse_down(MouseButton::Left,cx.listener(|this,_,_,cx|{
                    if let Some(windows)=&this.windows{if let Some((x,y))=native::drag(windows.floating){this.send(Command::Position(x,y));}}cx.notify();
                })).child(img("app.png").size(px(30.))).child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(white()).child("工时记录")))
                .child(div().flex().gap_1()
                    .child(Button::new("float-pin").ghost().label(if self.snapshot.settings.topmost{"📌"}else{"♧"}).on_click(cx.listener(|this,_,_,cx|{
                        let enabled=!this.snapshot.settings.topmost;this.snapshot.settings.topmost=enabled;if let Some(w)=&this.windows{native::pin(w.floating,enabled);}this.send(Command::Pin(enabled));cx.notify();
                    })))
                    .child(Button::new("float-hide").ghost().label("—").on_click(cx.listener(|this,_,_,_|{if let Some(w)=&this.windows{native::hide(w.floating);}})))))
            .child(div().flex_1().bg(rgb(0xf6f8fc).alpha(opacity)).px_4().py_2().flex().flex_col().gap_2()
                .child(div().flex().justify_between().text_xs().text_color(rgb(0x3276e8)).child(self.snapshot.status.clone())
                    .child(div().text_color(rgb(0x657893)).child(format!("{}—{}",self.snapshot.summary.period.start.format("%m/%d"),self.snapshot.summary.period.end.format("%m/%d")))))
                .child(div().flex().gap_4()
                    .child(div().flex_1().child(div().text_xs().text_color(rgb(0x657893)).child(title)).child(div().text_xl().font_weight(FontWeight::SEMIBOLD).mt_1().child(today_text)))
                    .child(div().flex_1().child(div().text_xs().text_color(rgb(0x657893)).child("当期总工时 · 含周末")).child(div().text_xl().font_weight(FontWeight::SEMIBOLD).mt_1().child(self.value(self.snapshot.summary.total)))))
                .child(div().text_xs().text_color(rgb(0x657893)).child(if self.snapshot.summary.has_data{exact(self.snapshot.summary.total)}else{"等待真实打卡数据".into()}))
                .child(div().flex().items_center().justify_between()
                    .child(div().text_xs().text_color(rgb(0x657893)).child(self.snapshot.cache.as_ref().map(|c|format!("{} 同步",c.last_sync.with_timezone(&china()).format("%m/%d %H:%M"))).unwrap_or_else(||"尚未同步".into())))
                    .child(div().flex().gap_1()
                        .child(Button::new("float-refresh").small().label("刷新").disabled(self.snapshot.busy||self.snapshot.needs_auth).on_click(cx.listener(|this,_,_,cx|{this.send(Command::Refresh);cx.notify();})))
                        .child(Button::new("float-main").small().label("主界面").on_click(cx.listener(|this,_,_,cx|{this.switch(false,cx);}))))))
            .into_any_element()
    }
}

impl Render for View {
    /// 功能：渲染当前显示模式；输入窗口和上下文，输出三页面壳或紧凑卡片。
    fn render(&mut self,_:&mut Window,cx:&mut Context<Self>)->impl IntoElement {
        // Modify:主窗口固定绘制主界面；旧共享实体分支导致小窗复用主窗口布局，保留原句便于追溯。
        // if self.floating{return self.floating_card(cx);}
        let pages=["主界面","每日打卡统计","设置配置"];
        let mut sidebar=div().w(px(180.)).flex_shrink_0().h_full().p_4().bg(cx.theme().secondary).flex().flex_col().gap_3()
            .child(div().flex().items_center().gap_2().mb_5().child(img("app.png").size(px(36.))).child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("工时记录")));
        for (index,label) in pages.iter().enumerate(){let button=Button::new(("nav",index)).label(*label).w_full().on_click(cx.listener(move|this,_,_,cx|{this.page=index;cx.notify();}));sidebar=sidebar.child(if self.page==index{button.primary()}else{button.ghost()});}
        sidebar=sidebar.child(div().flex_1()).child(div().text_xs().text_color(cx.theme().muted_foreground).child("WORK HOURS RECORD\nRust · 2.0.0"))
            .child(Button::new("exit").ghost().label("退出程序").on_click(cx.listener(|this,_,_,cx|{this.send(Command::Opacity(this.snapshot.settings.opacity));this.send(Command::Quit);cx.quit();})));
        let content=match self.page {0=>self.overview(cx),1=>self.statistics(cx),_=>self.settings(cx)};
        div().size_full().bg(cx.theme().background).text_color(cx.theme().foreground).font_family("Microsoft YaHei UI").text_sm().flex().child(sidebar)
            .child(div().flex_1().min_w_0().h_full().flex().flex_col()
                .child(div().p_5().border_b_1().border_color(cx.theme().border).flex().justify_between().items_center()
                    .child(div().flex().flex_col().gap_2().child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(pages[self.page]))
                        .child(div().text_sm().text_color(cx.theme().muted_foreground).child(format!("{} — {}",self.snapshot.summary.period.start,self.snapshot.summary.period.end))))
                    .child(div().flex().items_center().gap_3().child(div().text_xs().text_color(cx.theme().muted_foreground).child(self.snapshot.status.clone()))
                        .child(Button::new("refresh").label("刷新").disabled(self.snapshot.busy||self.snapshot.needs_auth).on_click(cx.listener(|this,_,_,cx|{this.send(Command::Refresh);cx.notify();})))
                        .child(Button::new("floating-mode").primary().label("悬浮模式").on_click(cx.listener(|this,_,_,cx|{this.switch(true,cx);}))))
                )
                .child(div().id("page-scroll").flex_1().min_h_0().overflow_y_scroll().p_5().child(content))
                .child(div().px_5().py_3().border_t_1().border_color(cx.theme().border).text_xs().text_color(cx.theme().muted_foreground)
                    .child(self.snapshot.cache.as_ref().map(|cache|format!("{} 同步 · {}",cache.last_sync.with_timezone(&china()).format("%m/%d %H:%M"),if self.snapshot.fresh{"已核验"}else{"缓存待更新"})).unwrap_or_else(||"尚未同步 · 首次请在设置中连接钉钉".into()))))
            .into_any_element()
    }
}
/// 功能：读取本应用GPUI窗口的Win32句柄；输入窗口，输出句柄或零。
fn hwnd(window:&Window)->usize {HasWindowHandle::window_handle(window).ok().and_then(|handle|match handle.as_raw(){RawWindowHandle::Win32(win)=>Some(win.hwnd.get() as usize),_=>None}).unwrap_or(0)}
/// 功能：启动独立主窗口、悬浮卡及消息桥；无输入，输出固定错误或在用户退出后结束。
pub fn run()->anyhow::Result<()> {
    let startup=std::time::Instant::now();
    let instance=native::Instance::acquire()?;let store=Store::open(None)?;let metrics_store=store.clone();let mut engine=Engine::start(store)?;let worker=engine.worker.take();let mut tray=native::Tray::new()?;
    let sender=engine.sender.clone();let exit_sender=sender.clone();let mut events=engine.events;
    application().with_assets(Assets).run(move|cx|{
        init(cx);let bounds=Bounds::centered(None,size(px(1100.),px(780.)),cx);
        let main_options=WindowOptions{window_bounds:Some(WindowBounds::Windowed(bounds)),window_min_size:Some(size(px(900.),px(650.))),titlebar:Some(TitlebarOptions{title:Some("工时记录 · Work Hours Record".into()),appears_transparent:false,..Default::default()}),..Default::default()};
        let (main,view)=open_window(main_options,cx,|window,cx|{
            let handle=hwnd(window);native::set_icon(handle);window.on_window_should_close(cx,|window,_|{native::hide(hwnd(window));false});
            cx.new(|cx|View::new(engine.initial.clone(),sender.clone(),window,cx))
        }).expect("工时记录主窗口创建失败");
        let main_hwnd=main.update(cx,|_,window,_|hwnd(window)).unwrap_or(0);
        // Modify:仅在本应用目录记录启动到主窗口创建耗时，供发布实测，不上传遥测。
        let _=metrics_store.write("startup.json",&serde_json::json!({"version":"2.0.0","measured_at":chrono::Utc::now(),"main_window_created_ms":startup.elapsed().as_millis()}));
        let card_options=WindowOptions{window_bounds:Some(WindowBounds::Windowed(Bounds::centered(None,size(px(368.),px(224.)),cx))),titlebar:None,kind:WindowKind::PopUp,
            window_background:WindowBackgroundAppearance::Transparent,is_resizable:false,is_minimizable:false,show:false,focus:false,..Default::default()};
        let shared=view.clone();
        let (card,_)=open_window(card_options,cx,move|window,cx|{
            native::set_icon(hwnd(window));native::transparent_border(hwnd(window));window.on_window_should_close(cx,|window,_|{native::hide(hwnd(window));false});
            cx.new(|cx|FloatingCard::new(shared,cx))
        }).expect("工时记录悬浮卡创建失败");
        // Modify:Kit根节点默认铺不透明底色；仅悬浮窗口清除此底色，保证背景滑块真正影响桌面合成。
        let card_hwnd=card.update(cx,|_,window,cx|{
            base::Root::update(window,cx,|root,_,cx|{root.style().background=Some(transparent_black().into());cx.notify();});hwnd(window)
        }).unwrap_or(0);
        view.update(cx,|this,cx|{native::place(card_hwnd,this.snapshot.settings.floating_position);native::pin(card_hwnd,this.snapshot.settings.topmost);this.windows=Some(native::Windows{main:main_hwnd,floating:card_hwnd});cx.notify();});
        cx.spawn(async move|cx|{
            loop{
                tokio::select!{
                    snapshot=events.recv()=>{
                        let Some(snapshot)=snapshot else{break;};
                        view.update(cx,|this,cx|{
                            let first_success=!this.snapshot.connection.connected_before&&snapshot.connection.connected_before&&snapshot.fresh;
                            if first_success{this.page=0;}else if snapshot.needs_auth&&!snapshot.busy&&this.snapshot.busy{this.page=2;}
                            this.snapshot=snapshot;cx.notify();
                        });
                    },
                    command=tray.events.recv()=>{
                        let Some(command)=command else{break;};
                        let _=view.update(cx,|this,cx|match command{
                            TrayCommand::Restore=>this.switch(this.floating,cx),TrayCommand::Main=>{this.page=0;this.switch(false,cx);},
                            TrayCommand::Floating=>this.switch(true,cx),TrayCommand::Refresh=>this.send(Command::Refresh),
                            TrayCommand::Quit=>{this.send(Command::Opacity(this.snapshot.settings.opacity));this.send(Command::Quit);cx.quit();},
                        });
                    }
                }
            }
            drop(tray);drop(instance);
        }).detach();
        cx.on_app_quit(move|_|{let _=exit_sender.send(Command::Quit);async{}}).detach();
    });if let Some(worker)=worker{let _=worker.join();}Ok(())
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use ::core::prelude::v1::test;
    use chrono::{Utc,TimeZone};
    /// 功能：构造一致的纯渲染夹具；无输入，输出不包含真人身份或授权的样本快照。
    fn fixture()->Snapshot {
        let now=Utc::now();let current=period(now);let date=today(now);let mut days=vec![];
        for day in dates(current.start,date){
            let start=china().from_local_datetime(&day.and_hms_opt(8,0,0).unwrap()).unwrap().timestamp_millis();
            let end=china().from_local_datetime(&day.and_hms_opt(19,0,0).unwrap()).unwrap().timestamp_millis();
            if day==date {
                let start=(now.timestamp_millis()-120*60000).max(china().from_local_datetime(&day.and_hms_opt(0,0,0).unwrap()).unwrap().timestamp_millis());
                days.push(AttendanceDay{date:day,records:vec![Punch{id:"fixture-today".into(),kind:PunchKind::OnDuty,time:start}],review:false,note:"离屏样本".into()});
            }else if !weekend(day){days.push(AttendanceDay{date:day,records:vec![Punch{id:format!("{day}-on"),kind:PunchKind::OnDuty,time:start},Punch{id:format!("{day}-off"),kind:PunchKind::OffDuty,time:end}],review:false,note:"离屏样本".into()});}
        }
        let cache=Cache{identity:"fixture:self".into(),user_id:"fixture".into(),group:"测试样本".into(),period:current.clone(),days,last_sync:now,complete:true};
        let summary=summarize(&current,&cache.days,now,true);let treasures=treasures(&summary,"fixture:self",&[]);
        Snapshot{settings:Default::default(),connection:work_hours_record::store::Connection{session:"fixture".into(),user:"fixture".into(),connected_before:true},cache:Some(cache),summary,treasures,busy:false,fresh:true,status:"离屏测试样本".into(),detail:"此页面使用隔离样本，不是真实钉钉数据。".into(),needs_auth:false}
    }
    #[test]
    /// 功能：检查真实GPUI场景的三页与独立卡片布局；不发送键鼠、不读取桌面、不接触用户应用或网络。
    fn three_pages_and_transparent_card_offscreen() {
        let output=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts/verification");std::fs::create_dir_all(&output).unwrap();
        let results=std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));let recorded=results.clone();
        // Modify:Windows后端禁止headless创建窗口；只创建show=false/focus=false的独立测试窗口，不显示或读取桌面。
        application().with_assets(Assets).run(move|cx|{
            init(cx);let (sender,_receiver)=tokio::sync::mpsc::unbounded_channel();
            let (handle,view)=open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(Bounds::centered(None,size(px(1100.),px(780.)),cx))),show:false,focus:false,..Default::default()},cx,
                |window,cx|cx.new(|cx|View::new(fixture(),sender,window,cx))).unwrap();
            // Modify:隐藏窗口的初始placement由后端延迟应用；显式设置内容区以触发真实布局，仍保持隐藏。
            handle.update(cx,|_,window,_|window.resize(size(px(1100.),px(780.)))).unwrap();
            cx.spawn(async move|cx|{
                for (page,name) in [(0,"main-fixture"),(1,"statistics-fixture"),(2,"settings-fixture")] {
                    view.update(cx,|view,cx|{view.page=page;view.floating=false;cx.notify();});
                    cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                    handle.update(cx,|_,window,cx|{
                        window.draw(cx).clear(cx);let viewport=window.viewport_size();let quads=window.painted_quads();
                        recorded.lock().unwrap().push(serde_json::json!({"page":name,"width":viewport.width.as_f32(),"height":viewport.height.as_f32(),"quads":quads.len(),"passed":viewport.width.as_f32()>=1099.&&quads.len()>5}));
                    }).unwrap();
                }
                let shared=view.clone();let card=cx.update(|cx|{
                    open_window(WindowOptions{window_bounds:Some(WindowBounds::Windowed(Bounds::centered(None,size(px(1100.),px(780.)),cx))),window_background:WindowBackgroundAppearance::Transparent,show:false,focus:false,titlebar:None,kind:WindowKind::PopUp,..Default::default()},cx,move|_,cx|cx.new(|cx|FloatingCard::new(shared,cx)))
                }).unwrap().0;
                view.update(cx,|view,cx|{view.floating=true;view.snapshot.settings.opacity=0.2;cx.notify();});
                // Modify:从主窗口大小开始验证原生尺寸修正，复现“小窗沿用主界面”而非只测试构造参数。
                card.update(cx,|_,window,_|native::place(hwnd(window),None)).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                card.update(cx,|_,window,cx|{
                    base::Root::update(window,cx,|root,_,cx|{root.style().background=Some(transparent_black().into());cx.notify();});
                    window.draw(cx).clear(cx);let viewport=window.viewport_size();let scale=window.scale_factor();let quads=window.painted_quads();
                    let navy:Hsla=rgb(0x1e304d).alpha(0.2).into();let light:Hsla=rgb(0xf6f8fc).alpha(0.2).into();
                    let header=quads.iter().find(|q|q.background.as_solid()==Some(navy));let body=quads.iter().find(|q|q.background.as_solid()==Some(light));
                    let header_size=header.map(|q|(q.bounds.size.width.0/scale,q.bounds.size.height.0/scale));
                    let body_size=body.map(|q|(q.bounds.size.width.0/scale,q.bounds.size.height.0/scale));
                    let passed=(viewport.width.as_f32()-368.).abs()<1.&&(viewport.height.as_f32()-224.).abs()<1.
                        &&header_size.is_some_and(|(w,h)|(w-368.).abs()<1.&&(h-52.).abs()<1.)&&body_size.is_some_and(|(w,h)|(w-368.).abs()<1.&&h<=173.);
                    recorded.lock().unwrap().push(serde_json::json!({"page":"floating-20-fixture","width":viewport.width.as_f32(),"height":viewport.height.as_f32(),"header_size":header_size,"body_size":body_size,"background_opacity":0.2,"passed":passed}));
                }).unwrap();
                // Modify:交替绘制两窗口，确认悬浮开关不再替换主窗口内容，也不串用窗口布局。
                handle.update(cx,|_,window,cx|{window.draw(cx).clear(cx);let width=window.viewport_size().width.as_f32();let scale=window.scale_factor();
                    let sidebar=window.painted_quads().iter().any(|q|(q.bounds.size.width.0/scale-180.).abs()<1.&&(q.bounds.size.height.0/scale-780.).abs()<1.);
                    recorded.lock().unwrap().push(serde_json::json!({"page":"main-after-floating","width":width,"has_sidebar":sidebar,"passed":width>=1099.&&sidebar}));}).unwrap();
                let _=cx.update(|cx|cx.quit());
            }).detach();
        });
        // Modify:Kit Windows包装层未提供像素回读，验证真实绘制场景而不将失败截图冒充视觉验收。
        let results=results.lock().unwrap();std::fs::write(output.join("layout-checks.json"),serde_json::to_vec_pretty(&*results).unwrap()).unwrap();
        assert_eq!(results.len(),5);assert!(results.iter().all(|r|r["passed"]==true),"布局验证失败：{results:?}");
    }
}
