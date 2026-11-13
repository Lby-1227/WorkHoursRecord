// Modify:单轮异步同步、共享快照、分钟计时及本地领取；界面切换不创建新连接。$saving-ai-agent-tokens
use crate::{attendance::AttendanceClient, domain::*, store::{Store, Settings, Connection}};
use anyhow::Result;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub settings: Settings, pub connection: Connection, pub cache: Option<Cache>, pub summary: Summary,
    pub treasures: Vec<Treasure>, pub busy: bool, pub fresh: bool, pub status: String, pub detail: String,
    pub needs_auth: bool,
}
pub enum Command {
    Connect, Refresh, Cancel, Claim(Category, usize), Opacity(f32), Pin(bool), Position(f32, f32), Quit,
}
pub struct Engine { pub sender: mpsc::UnboundedSender<Command>, pub events: mpsc::UnboundedReceiver<Snapshot>, pub initial: Snapshot, pub worker: Option<std::thread::JoinHandle<()>> }
struct Completion { result: std::result::Result<(AttendanceClient, Cache), String>, requested_date: chrono::NaiveDate }

impl Snapshot {
    /// 功能：取得今日汇总；无输入，输出真实今天的行。
    pub fn today(&self) -> Option<&DaySummary> { self.summary.days.iter().find(|day| day.date == today(Utc::now())) }
    /// 功能：取得当前宝箱每日目标；输入类别，输出已确认口径下的中文预测或状态。
    pub fn plan(&self, category: Category) -> String {
        if !self.summary.has_data { return "读取考勤后显示每日目标".into(); }
        if !self.summary.reliable { return "考勤待更新或核对，暂不分摊每日目标".into(); }
        let Some(target) = visible(&self.treasures, category) else { return "—".into(); };
        let remaining = (target.required - target.current).max(0);
        if remaining == 0 { return "当前宝箱目标已达到".into(); }
        let closed = self.today().is_some_and(|day| day.state == DayState::Complete);
        let (days, minutes) = pace(&self.summary.period, Utc::now(), closed, remaining, category == Category::Weekday);
        let unit = if category == Category::Total { "自然日（含周末）" } else { "工作日" };
        if days == 0 { return format!("本周期已无剩余{unit}"); }
        let action = if category == Category::Total { "累计净工时" } else { "加班" };
        format!("剩余{days}个{unit} · 平均每天还需{action}{}（按分钟向上取整）", duration_text(minutes))
    }
}

impl Engine {
    /// 功能：启动唯一后台运行时；输入本应用存储，输出命令发送器和按需快照事件。
    pub fn start(store: Arc<Store>) -> Result<Self> {
        let settings = store.settings()?; let connection = store.connection()?; let claims = store.claims()?;
        let now = Utc::now(); let period = period(now);
        let identity = format!("{}:{}", connection.session, connection.user);
        let cache = if store.has_tokens() && !connection.user.is_empty() { store.cache(&identity, &period)? } else { None };
        let clock = cache.as_ref().filter(|cache| today(cache.last_sync) == today(now)).map_or(now, |cache| cache.last_sync);
        let summary = summarize(&period, cache.as_ref().map_or(&[], |cache| cache.days.as_slice()), clock, false);
        let treasures = treasures(&summary, &identity, &claims);
        let initial = Snapshot { settings, connection, cache, summary, treasures, busy: false, fresh: false,
            status: "未连接".into(), detail: "在设置中连接官方钉钉考勤，读取本人真实打卡记录。".into(), needs_auth: !store.has_tokens() };
        let (sender, receiver) = mpsc::unbounded_channel(); let (events_sender, events) = mpsc::unbounded_channel();
        let background = initial.clone();
        // Modify:运行时创建失败直接报告，正常退出等待后台保存完成，避免退出瞬间丢失最后的外观设置。
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
        let worker = std::thread::Builder::new().name("attendance-runtime".into()).spawn(move || {
            runtime.block_on(run(store, background, claims, receiver, events_sender));
        })?;
        Ok(Self { sender, events, initial, worker: Some(worker) })
    }
}

/// 功能：根据可信快照更新汇总；输入可变状态和领取历史，无输出，不产生网络请求。
fn recalculate(state: &mut Snapshot, claims: &[Claim]) {
    let now = Utc::now(); let period = period(now);
    if state.cache.as_ref().is_some_and(|cache| cache.period != period) { state.cache = None; state.fresh = false; }
    let clock = if !state.fresh { state.cache.as_ref().filter(|c| today(c.last_sync) == today(now)).map_or(now, |c| c.last_sync) } else { now };
    state.summary = summarize(&period, state.cache.as_ref().map_or(&[], |cache| cache.days.as_slice()), clock,
        state.fresh && state.cache.as_ref().is_some_and(|cache| cache.complete));
    let identity = format!("{}:{}", state.connection.session, state.connection.user);
    state.treasures = treasures(&state.summary, &identity, claims);
    if state.fresh && state.summary.days.iter().any(|day| day.state == DayState::NeedsReview) {
        state.detail = "存在缺卡、多段或跨日记录，已标为待核对；这些日期不计入，宝箱暂不可领取。".into();
    }
}

/// 功能：后台事件循环；输入状态、存储及通道，输出通过快照事件传递，网络任务可取消且不并发。
async fn run(store: Arc<Store>, mut state: Snapshot, mut claims: Vec<Claim>, mut commands: mpsc::UnboundedReceiver<Command>, events: mpsc::UnboundedSender<Snapshot>) {
    let mut client: Option<AttendanceClient> = None; let mut active: Option<CancellationToken> = None;
    let (finished_tx, mut finished_rx) = mpsc::unbounded_channel::<Completion>();
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
    let mut last_date = today(Utc::now()); let mut last_attempt = Utc::now() - chrono::Duration::minutes(5);
    let mut pending_full = store.has_tokens(); let mut request: Option<(bool, bool)> = None;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    Command::Quit => { if let Some(token) = &active { token.cancel(); } break; },
                    Command::Cancel => { if let Some(token) = &active { token.cancel(); } },
                    Command::Connect if !state.busy => {
                        state.connection = Connection::default(); state.cache = None; state.fresh = false;
                        if store.write("connection.json", &state.connection).is_ok() { request = Some((true, true)); }
                        else { state.detail = "连接分区保存失败，未开始授权。".into(); }
                    },
                    Command::Refresh if !state.busy => { request = Some((true, false)); },
                    Command::Claim(category, level) if !state.busy => {
                        recalculate(&mut state, &claims); let previous = claims.clone();
                        let identity = format!("{}:{}", state.connection.session, state.connection.user);
                        if claim(&mut claims, &state.summary, &identity, category, level, Utc::now()) && store.write("claims.json", &claims).is_err() {
                            claims = previous; state.detail = "领取记录保存失败，本次领取已回滚。".into();
                        }
                    },
                    Command::Opacity(value) => {
                        if value.is_finite() { state.settings.opacity = (value * 100.).round().clamp(20.,100.) / 100.; }
                        if store.write("settings.json", &state.settings).is_err() { state.detail = "外观设置保存失败。".into(); }
                    },
                    Command::Pin(value) => { state.settings.topmost = value; if store.write("settings.json", &state.settings).is_err() { state.detail = "置顶设置保存失败。".into(); } },
                    Command::Position(x,y) => { if x.is_finite() && y.is_finite() { state.settings.floating_position = Some((x,y)); let _ = store.write("settings.json", &state.settings); } },
                    _ => {},
                }
            },
            Some(completion) = finished_rx.recv() => {
                state.busy = false; active = None;
                match completion.result {
                    Ok((connected, cache)) if completion.requested_date == today(Utc::now()) => {
                        let mut connection = state.connection.clone(); connection.user = cache.user_id.clone(); connection.connected_before = true;
                        if store.save_cache(&cache).and_then(|_| store.write("connection.json", &connection)).is_ok() {
                            state.connection = connection; state.cache = Some(cache); state.fresh = true; state.needs_auth = false;
                            state.status = "已连接".into(); state.detail = "今日从上班打卡开始计时；读取下班卡后校准并停止，多算部分自动减回。".into(); client = Some(connected);
                        } else { state.fresh = false; state.status = "保存失败".into(); state.detail = "本轮数据未能保存，请检查用户目录。".into(); }
                    },
                    Ok(_) => { pending_full = true; state.fresh = false; state.status = "日期已切换".into(); state.detail = "正在重新同步当期。".into(); },
                    Err(error) => {
                        state.fresh = false; state.status = "同步未完成".into();
                        state.needs_auth = error.contains("授权") || !store.has_tokens(); state.detail = error; client = None;
                    }
                }
            },
            _ = timer.tick() => {
                let date = today(Utc::now()); if date != last_date { pending_full = true; last_date = date; }
                if !state.busy && store.has_tokens() && (pending_full || Utc::now() - last_attempt >= chrono::Duration::minutes(5)) {
                    request = Some((pending_full || state.cache.is_none(), false));
                }
            },
        }
        if !state.busy && pending_full && store.has_tokens() && request.is_none() { request = Some((true, false)); }
        if let Some((full, interactive)) = request.take() {
            pending_full = false; state.busy = true; state.status = if interactive { "等待授权 / 连接中" } else { "同步中" }.into();
            state.detail = if interactive { "请在系统浏览器完成本人账号和组织的授权，最多等待5分钟。" } else { "正在读取本人考勤并核对完整性。" }.into();
            let cancel = CancellationToken::new(); active = Some(cancel.clone()); last_attempt = Utc::now();
            let current_client = client.take(); let store = store.clone(); let sender = finished_tx.clone();
            let cache = state.cache.clone(); let connection = state.connection.clone(); let requested_date = today(Utc::now());
            // Modify:耗时网络在独立任务，事件循环仍能立即处理取消、外观和退出；一轮完成前不接受第二轮同步。
            tokio::spawn(async move {
                let work = async move {
                    let connected = if interactive || current_client.is_none() {
                        tokio::time::timeout(std::time::Duration::from_secs(if interactive {300} else {30}), AttendanceClient::connect(store, interactive)).await
                            .map_err(|_| anyhow::anyhow!("连接或授权已超时，请重试。"))??
                    } else { current_client.unwrap() };
                    let read = async {
                        let now = Utc::now(); let period = period(now);
                        if full || cache.as_ref().is_none_or(|cache| cache.period != period) {
                            connected.read_period(&period, now, &connection.session).await
                        } else {
                            let mut cache = cache.unwrap(); let (user, group, day) = connected.personal(today(now)).await?;
                            anyhow::ensure!(user == cache.user_id && cache.identity == format!("{}:{user}", connection.session), "本人身份与缓存不一致，请重新连接。");
                            cache.days.retain(|existing| existing.date != day.date); cache.days.push(day);
                            cache.group = group; cache.last_sync = Utc::now(); Ok(cache)
                        }
                    };
                    let cache = tokio::time::timeout(std::time::Duration::from_secs(30), read).await.map_err(|_| anyhow::anyhow!("考勤读取已超时，请刷新重试。"))??;
                    Ok::<_, anyhow::Error>((connected, cache))
                };
                // Modify:普通连接和读取共用30秒总预算；用户浏览器授权另有5分钟，后续读取仍限30秒。
                let result = tokio::select! {
                    _ = cancel.cancelled() => Err("本轮操作已取消，保留上次快照。".to_string()),
                    result = tokio::time::timeout(std::time::Duration::from_secs(if interactive {330} else {30}), work) => {
                        match result { Ok(result) => result.map_err(|error| error.to_string()), Err(_) => Err("同步已超时，保留上次快照。".into()) }
                    }
                };
                let _ = sender.send(Completion { result, requested_date });
            });
        }
        recalculate(&mut state, &claims); if events.send(state.clone()).is_err() { break; }
    }
}
