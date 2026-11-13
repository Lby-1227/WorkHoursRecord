// Modify:只读复核Rust应用本人快照，不输出凭证或本人ID，仅保存验收结果。$saving-ai-agent-tokens
use work_hours_record::{domain::*,store::Store};
use chrono::{Utc,Duration};
use serde_json::json;
/// 功能：核对独立授权后的真实快照与计时；无参数，输出不含凭证和身份原文的本地JSON报告。
fn main()->anyhow::Result<()> {
    let store=Store::open(None)?;let connection=store.connection()?;let now=Utc::now();let period=period(now);
    let identity=format!("{}:{}",connection.session,connection.user);
    let cache=store.cache(&identity,&period)?.ok_or_else(||anyhow::anyhow!("当前Rust授权尚无当期快照。"))?;
    anyhow::ensure!(cache.user_id==connection.user&&cache.identity==identity&&cache.complete,"缓存身份或完整性未通过。");
    // Modify:本工具不发起实时读取，按快照时刻复算，避免网络失效时把旧上班卡继续外推成真实新工时。
    // let summary=summarize(&period,&cache.days,now,true);
    let calculated_at=cache.last_sync;let summary=summarize(&period,&cache.days,calculated_at,true);let claims=store.claims()?;let treasures=treasures(&summary,&identity,&claims);
    let current=summary.days.iter().find(|day|day.date==today(now));let closed=current.is_some_and(|day|day.state==DayState::Complete);
    let plans:Vec<_>=[Category::Total,Category::Weekday].iter().map(|category|{
        let target=visible(&treasures,*category).unwrap();let remaining=(target.required-target.current).max(0);
        let (days,minutes)=pace(&period,calculated_at,closed,remaining,*category==Category::Weekday);
        json!({"category":format!("{category:?}"),"level":target.level,"remaining_minutes":remaining,"remaining_days":days,"daily_required_minutes":minutes,"today_closed":closed})
    }).collect();
    let historical_stops=cache.days.iter().filter(|day|calculate_day(day,now).state==DayState::Complete).all(|day|calculate_day(day,now).minutes==calculate_day(day,now+Duration::minutes(2)).minutes);
    let report=json!({"checked_at_china":now.with_timezone(&china()),"last_sync_china":cache.last_sync.with_timezone(&china()),"calculated_at_china":calculated_at.with_timezone(&china()),"live_read_performed":false,"cache_age_seconds":(now-cache.last_sync).num_seconds(),"identity_matches":true,"data_complete":cache.complete,
        "period":{"start":period.start,"end":period.end},"total_minutes":summary.total,"total_hours":summary.total as f64/60.,"weekday_minutes":summary.weekday,"weekend_minutes":summary.weekend,"weekday_overtime_minutes":summary.overtime,
        "today":current.map(|day|json!({"date":day.date,"on":punch_text(day.on),"off":punch_text(day.off),"minutes":day.minutes,"state":state_text(day.state)})),
        "review_days":summary.days.iter().filter(|day|day.state==DayState::NeedsReview).count(),"daily_plans":plans,"completed_days_do_not_grow":historical_stops,
        "live_off_duty_transition_observed":false,"note":"按最后成功的真实快照时刻复算，本工具不发起实时读取；已下班卡在后续时刻不增长，现场下班转换尚未观察。"});
    let output=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts");std::fs::create_dir_all(&output)?;
    std::fs::write(output.join("real-integration.json"),serde_json::to_vec_pretty(&report)?)?;println!("{}",serde_json::to_string_pretty(&report)?);Ok(())
}
