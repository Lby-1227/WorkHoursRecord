// Modify:北京时间分钟计算、周期、两类宝箱及每日预测；纯函数不访问网络或磁盘。$saving-ai-agent-tokens
use chrono::{DateTime, Datelike, Days, Duration, FixedOffset, NaiveDate, TimeZone, Utc, Weekday};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PunchKind { Unknown, OnDuty, OffDuty }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Punch { pub id: String, pub kind: PunchKind, pub time: i64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttendanceDay { pub date: NaiveDate, pub records: Vec<Punch>, pub review: bool, pub note: String }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DayState { NoRecords, Complete, InProgress, NeedsReview, Future }
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Period { pub start: NaiveDate, pub end: NaiveDate, pub working_days: u32 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaySummary {
    pub date: NaiveDate, pub on: Option<i64>, pub off: Option<i64>, pub raw: Option<i64>,
    pub rest: i64, pub minutes: Option<i64>, pub overtime: Option<i64>,
    pub state: DayState, pub weekend: bool, pub note: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cache {
    pub identity: String, pub user_id: String, pub group: String, pub period: Period,
    pub days: Vec<AttendanceDay>, pub last_sync: DateTime<Utc>, pub complete: bool,
}
#[derive(Clone, Debug)]
pub struct Summary {
    pub period: Period, pub days: Vec<DaySummary>, pub total: i64, pub weekday: i64,
    pub weekend: i64, pub overtime: i64, pub has_data: bool, pub reliable: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category { Total, Weekday }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claim { pub key: String, pub claimed_at: DateTime<Utc>, pub running: bool }
#[derive(Clone, Debug)]
pub struct Treasure {
    pub category: Category, pub level: usize, pub award: i64, pub required: i64, pub current: i64,
    pub reached: bool, pub claimed: bool, pub can_claim: bool,
}

/// 功能：返回固定北京时间偏移；无输入，输出UTC+8，不依赖电脑时区。
pub fn china() -> FixedOffset { FixedOffset::east_opt(8 * 3600).unwrap() }
/// 功能：取得当前北京时间日期；输入UTC时刻，输出本地日期。
pub fn today(now: DateTime<Utc>) -> NaiveDate { now.with_timezone(&china()).date_naive() }
/// 功能：识别周六日；输入日期，输出是否按周末规则计算。
pub fn weekend(date: NaiveDate) -> bool { matches!(date.weekday(), Weekday::Sat | Weekday::Sun) }
/// 功能：生成日期序列；输入包含首尾的区间，输出顺序日期。
pub fn dates(start: NaiveDate, end: NaiveDate) -> impl Iterator<Item=NaiveDate> {
    (0..=(end - start).num_days().max(-1)).map(move |offset| start + Duration::days(offset))
}
/// 功能：取得25日至次月24日周期；输入UTC时刻，输出完整周期及工作日数量。
pub fn period(now: DateTime<Utc>) -> Period {
    let date = today(now);
    let anchor = if date.day() >= 25 { date } else { date.with_day(1).unwrap() - Duration::days(1) };
    let start = anchor.with_day(25).unwrap();
    let next = (start.with_day(1).unwrap() + Duration::days(32)).with_day(1).unwrap();
    let end = next.with_day(24).unwrap();
    let working_days = dates(start, end).filter(|date| !weekend(*date)).count() as u32;
    Period { start, end, working_days }
}
/// 功能：构造无记录快照；输入日期，输出不带推断打卡的空记录。
pub fn empty_day(date: NaiveDate) -> AttendanceDay { AttendanceDay { date, records: vec![], review: false, note: String::new() } }
/// 功能：计算单日净分钟；输入日快照和当前UTC时刻，输出准确分钟或待核对状态。
pub fn calculate_day(day: &AttendanceDay, now: DateTime<Utc>) -> DaySummary {
    let is_weekend = weekend(day.date);
    let mut result = DaySummary { date: day.date, on: None, off: None, raw: None, rest: 0,
        minutes: None, overtime: None, state: DayState::NoRecords, weekend: is_weekend, note: day.note.clone() };
    if day.date > today(now) { result.state = DayState::Future; result.note = "尚未发生".into(); return result; }
    let mut unique = HashSet::new();
    let records: Vec<_> = day.records.iter().filter(|p| unique.insert((p.kind, p.time))).collect();
    let starts: Vec<_> = records.iter().filter(|p| p.kind == PunchKind::OnDuty).collect();
    let ends: Vec<_> = records.iter().filter(|p| p.kind == PunchKind::OffDuty).collect();
    result.on = starts.first().map(|p| p.time); result.off = ends.first().map(|p| p.time);
    if records.is_empty() && !day.review { return result; }
    result.state = DayState::NeedsReview;
    // Modify:未知类型、多个班次和明确异常不自行选最早/最晚时间配对。
    if day.review || records.iter().any(|p| p.kind == PunchKind::Unknown) || starts.len() != 1 || ends.len() > 1 {
        if result.note.is_empty() { result.note = "缺少唯一上下班配对，需核对".into(); } return result;
    }
    let running = day.date == today(now) && ends.is_empty();
    if !running && ends.is_empty() { result.note = "缺少下班卡或跨日未结束".into(); return result; }
    let start = starts[0].time;
    let finish = if running { now.timestamp_millis() } else { ends[0].time };
    if start > finish || finish > now.timestamp_millis() { result.note = "打卡时间顺序异常或晚于当前时刻".into(); return result; }
    let Some(start_time) = Utc.timestamp_millis_opt(start).single() else { result.note = "时间超出有效范围".into(); return result; };
    let Some(finish_time) = Utc.timestamp_millis_opt(finish).single() else { result.note = "时间超出有效范围".into(); return result; };
    if today(start_time) != day.date || today(finish_time) != day.date { result.note = "跨日班次需核对".into(); return result; }
    let span = finish - start;
    let rest_ms = if is_weekend {
        if span > 10 * 3600_000 { 120 * 60_000 } else if span > 6 * 3600_000 { 60 * 60_000 } else { 0 }
    } else {
        // Modify:先以毫秒计算午休交集，再舍去净工时不足一分钟，避免部分午休多计。
        let lunch = china().from_local_datetime(&day.date.and_hms_opt(12, 0, 0).unwrap()).single().unwrap().timestamp_millis();
        (finish.min(lunch + 3600_000) - start.max(lunch)).max(0)
    };
    let raw = span / 60_000; let net = (span - rest_ms) / 60_000;
    result.raw = Some(raw); result.rest = raw - net; result.minutes = Some(net);
    result.overtime = if is_weekend { None } else { Some((net - 480).max(0)) };
    result.state = if running { DayState::InProgress } else { DayState::Complete };
    result
}
/// 功能：汇总整个周期；输入按日快照、UTC时刻及读取完整性，输出汇总和可领取可信度。
pub fn summarize(period: &Period, source: &[AttendanceDay], now: DateTime<Utc>, complete: bool) -> Summary {
    // Modify:相同日期最后快照覆盖，避免同步与重启重复累计。
    let map: BTreeMap<_, _> = source.iter().map(|day| (day.date, day)).collect();
    let days: Vec<_> = dates(period.start, period.end).map(|date| {
        calculate_day(map.get(&date).copied().unwrap_or(&empty_day(date)), now)
    }).collect();
    let total = days.iter().filter_map(|day| day.minutes).sum();
    let weekday = days.iter().filter(|day| !day.weekend).filter_map(|day| day.minutes).sum();
    let overtime = days.iter().filter_map(|day| day.overtime).sum();
    let has_data = days.iter().any(|day| day.minutes.is_some());
    let reliable = complete && has_data && days.iter().all(|day| day.state != DayState::NeedsReview);
    Summary { period: period.clone(), days, total, weekday, weekend: total - weekday, overtime, has_data, reliable }
}
/// 功能：形成身份和周期隔离的领取键；输入身份、周期、类别和档位，输出稳定键。
pub fn claim_key(identity: &str, period: &Period, category: Category, level: usize) -> String {
    format!("{identity}|{}|{category:?}|{level}", period.start.format("%Y%m%d"))
}
/// 功能：评估六档进度及顺序领取；输入可信汇总、身份及领取记录，输出六档状态。
pub fn treasures(summary: &Summary, identity: &str, claims: &[Claim]) -> Vec<Treasure> {
    let mut result = Vec::with_capacity(6);
    // Modify:两类独立逐档解锁；金额评估取同类最高档，不累计低档金额。
    for (category, targets, awards, current) in [
        (Category::Total, [15001, 16801, 19201], [300, 500, 1000], summary.total),
        (Category::Weekday, [1800, 3000, 4200], [500, 1000, 1500], summary.overtime),
    ] {
        let mut prior = true;
        for index in 0..3 {
            let claimed = claims.iter().any(|claim| claim.key == claim_key(identity, &summary.period, category, index + 1));
            let reached = current >= targets[index];
            result.push(Treasure { category, level: index + 1, award: awards[index], required: targets[index], current,
                reached, claimed, can_claim: prior && !claimed && reached && summary.reliable });
            prior &= claimed;
        }
    }
    result
}
/// 功能：取得当前可见档位；输入六档和类别，输出首个未领取或最后档位。
pub fn visible(treasures: &[Treasure], category: Category) -> Option<&Treasure> {
    treasures.iter().find(|t| t.category == category && !t.claimed)
        .or_else(|| treasures.iter().rev().find(|t| t.category == category))
}
/// 功能：登记一次幂等领取；输入最新汇总、身份、类别档位及时间，输出是否新增标记。
pub fn claim(claims: &mut Vec<Claim>, summary: &Summary, identity: &str, category: Category, level: usize, now: DateTime<Utc>) -> bool {
    let all = treasures(summary, identity, claims);
    if !visible(&all, category).is_some_and(|t| t.level == level && t.can_claim) { return false; }
    claims.push(Claim { key: claim_key(identity, &summary.period, category, level), claimed_at: now,
        running: summary.days.iter().any(|day| day.state == DayState::InProgress) }); true
}
/// 功能：分摊每日剩余目标；输入周期、时刻、今日下班状态、剩余分钟及是否只含工作日，输出日数与向上取整分钟。
pub fn pace(period: &Period, now: DateTime<Utc>, closed: bool, remaining: i64, weekdays_only: bool) -> (usize, i64) {
    let date = today(now); let start = if closed { date + Days::new(1) } else { date };
    let count = dates(start.max(period.start), period.end).filter(|d| !weekdays_only || !weekend(*d)).count();
    if count == 0 { return (0, 0); }
    let minutes = remaining.max(0); let days = count as i64;
    (count, minutes / days + i64::from(minutes % days != 0))
}
/// 功能：格式化净分钟；输入整数分钟，输出小时分钟，不用小时浮点值累计。
pub fn duration_text(minutes: i64) -> String { format!("{}小时{:02}分", minutes / 60, minutes % 60) }
/// 功能：格式化精确分钟与小时；输入分钟，输出四位小数的显示文本。
pub fn exact(minutes: i64) -> String { format!("{minutes} 分钟 / {:.4} 小时", minutes as f64 / 60.) }
/// 功能：格式化实际打卡时间；输入可空毫秒，输出北京时间或空值占位。
pub fn punch_text(milliseconds: Option<i64>) -> String {
    milliseconds.and_then(|value| Utc.timestamp_millis_opt(value).single())
        .map(|time| time.with_timezone(&china()).format("%H:%M:%S").to_string()).unwrap_or_else(|| "—".into())
}
/// 功能：显示每日状态；输入状态，输出稳定中文文案。
pub fn state_text(state: DayState) -> &'static str {
    match state { DayState::NoRecords => "无记录", DayState::Complete => "已下班", DayState::InProgress => "计时中", DayState::NeedsReview => "待核对", DayState::Future => "未到日期" }
}
