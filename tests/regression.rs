// Modify:WPF基准的分钟、异常解析、宝箱、预测及独立存储回归，全部使用隔离夹具。$saving-ai-agent-tokens
use work_hours_record::{domain::*, parser::*, store::{Store, Settings, Connection, protect}, attendance::{decode,validate_schema,TOOLS}};
use chrono::{DateTime,NaiveDate,TimeZone,Utc,Duration};
use serde_json::{json,Value};

/// 功能：解析固定北京时间测试时刻；输入时间文本，输出UTC时刻，不读取系统时钟。
fn clock(value:&str)->DateTime<Utc>{DateTime::parse_from_rfc3339(&format!("{}+08:00",value.replace(' ',"T"))).unwrap().with_timezone(&Utc)}
/// 功能：解析夹具日期；输入日期文本，输出日期。
fn date(value:&str)->NaiveDate{value.parse().unwrap()}
/// 功能：构造明确上下班测试卡；输入日期与两个时间，输出日快照。
fn day(value:&str,on:&str,off:&str)->AttendanceDay{AttendanceDay{date:date(value),records:vec![
    Punch{id:"on".into(),kind:PunchKind::OnDuty,time:clock(&format!("{value} {on}")).timestamp_millis()},
    Punch{id:"off".into(),kind:PunchKind::OffDuty,time:clock(&format!("{value} {off}")).timestamp_millis()}],review:false,note:String::new()}}
/// 功能：生成只含上班卡的夹具；输入日期时间，输出未下班日快照。
fn open_day(value:&str,on:&str)->AttendanceDay{let mut day=day(value,on,on);day.records.pop();day}
/// 功能：创建用于门槛测试的汇总；输入总分钟、工作日加班及可信度，输出完整测试汇总。
fn synthetic(total:i64,overtime:i64,reliable:bool)->Summary{Summary{period:period(clock("2026-11-06 20:00:00")),days:vec![],total,weekday:total,weekend:0,overtime,has_data:true,reliable}}
/// 功能：构造个人接口回执；输入记录数组，输出带本人标识的官方字段夹具。
fn personal_response(records:Value)->Value{json!({"success":true,"result":{"currentUserId":"self","groupName":"test","recordList":records}})}
/// 功能：构造同一毫秒的人工个人卡；输入类型和正常状态，输出官方字段对象，不使用本人打卡时刻。
// Modify:日期与解析用例对齐，时刻使用人工整点；隐私原值不保留在注释或公开提交中。$saving-ai-agent-tokens
fn card(kind:Value,normal:bool)->Value{json!({"checkTypeDesc":kind,"isNormal":normal,"userCheckTime":clock("2026-11-09 09:00:00").timestamp_millis()})}
/// 功能：构造流水和判定夹具；无输入，输出两个可关联的打卡数组。
fn batch()->(Vec<Value>,Vec<Value>){
    let millis=clock("2026-11-02 08:00:00").timestamp_millis();let work=clock("2026-11-02 00:00:00").timestamp_millis();
    (vec![json!({"id":"raw1","userId":"self","userCheckTime":millis,"checkType":"OnDuty"})],
     vec![json!({"id":"result1","recordId":"raw1","userId":"self","userCheckTime":millis,"workDate":work,"checkType":"OnDuty","timeResult":"Normal"})])
}
/// 功能：构造官方工具Schema；输入工具下标，输出该工具已核对参数结构。
fn schema(index:usize)->Value{match index{
    0=>json!({"type":"object","properties":{"workDate":{"type":"number"}},"required":["workDate"]}),
    1=>json!({"type":"object","properties":{"QueryCheckRecordRequest":{"type":"object","properties":{"userIds":{"type":"array"},"checkDateFrom":{"type":"string"},"checkDateTo":{"type":"string"}},"required":["userIds","checkDateFrom","checkDateTo"]}},"required":["QueryCheckRecordRequest"]}),
    _=>json!({"type":"object","properties":{"QueryCheckResultRequest":{"type":"object","properties":{"userIds":{"type":"array"},"workDateFrom":{"type":"string"},"workDateTo":{"type":"string"},"offset":{"type":"number"},"limit":{"type":"number"}},"required":["userIds","workDateFrom","workDateTo","offset","limit"]}},"required":["QueryCheckResultRequest"]})}}
macro_rules! check {($name:ident,$body:block)=>{#[test] #[doc="Modify:隔离回归用例；无输入，失败时报告断言。"] fn $name() $body};}
macro_rules! minutes {($name:ident,$date:literal,$on:literal,$off:literal,$raw:expr,$net:expr,$rest:expr,$ot:expr)=>{
    check!($name,{let result=calculate_day(&day($date,$on,$off),clock("2026-11-09 23:59:59"));assert_eq!(result.raw,Some($raw));assert_eq!(result.minutes,Some($net));assert_eq!(result.rest,$rest);assert_eq!(result.overtime,$ot);assert_eq!(result.state,DayState::Complete);});};}
// Modify:用人工日期和时刻覆盖相同的627分58秒跨度、午休及逐日取整，避免公开原始个人样本。
minutes!(synthetic_seconds_floor,"2026-11-02","09:10:11","19:38:09",627,567,60,Some(87));
minutes!(eight_hours_after_lunch,"2026-11-02","08:00:00","17:00:00",540,480,60,Some(0));
minutes!(first_overtime_minute,"2026-11-02","08:00:00","17:01:00",541,481,60,Some(1));
minutes!(weekday_eleven_hour_span,"2026-11-02","08:00:00","19:00:00",660,600,60,Some(120));
minutes!(no_lunch_when_ending_at_noon,"2026-11-02","08:00:00","12:00:00",240,240,0,Some(0));
minutes!(no_lunch_when_starting_at_one,"2026-11-02","13:00:00","15:00:00",120,120,0,Some(0));
minutes!(afternoon_only,"2026-11-02","15:00:00","19:00:00",240,240,0,Some(0));
minutes!(partial_lunch_late_start,"2026-11-02","12:30:00","14:00:00",90,60,30,Some(0));
minutes!(partial_lunch_early_finish,"2026-11-02","11:00:00","12:30:00",90,60,30,Some(0));
minutes!(inside_lunch_zero,"2026-11-02","12:15:00","12:45:00",30,0,30,Some(0));
minutes!(full_lunch_overlap,"2026-11-02","11:00:00","14:00:00",180,120,60,Some(0));
minutes!(one_net_second_is_zero_minutes,"2026-11-02","12:00:01","13:00:01",60,0,60,Some(0));
minutes!(partial_lunch_seconds_before_floor,"2026-11-02","12:30:01","13:01:00",30,1,29,Some(0));
minutes!(weekend_exact_six_hours,"2026-10-31","08:00:00","14:00:00",360,360,0,None);
minutes!(weekend_six_hours_one_second,"2026-10-31","08:00:00","14:00:01",360,300,60,None);
minutes!(weekend_exact_ten_hours,"2026-10-31","08:00:00","18:00:00",600,540,60,None);
minutes!(weekend_ten_hours_one_second,"2026-10-31","08:00:00","18:00:01",600,480,120,None);
minutes!(weekend_no_extra_lunch,"2026-10-31","11:00:00","14:00:00",180,180,0,None);
minutes!(weekend_two_hours_not_three,"2026-10-31","08:00:00","19:00:00",660,540,120,None);
// Modify:公开源码使用人工构造的时间，保留秒差与午休计算覆盖；原始个人样本仅留在本地工程。$saving-ai-agent-tokens
minutes!(synthetic_weekday_seconds,"2026-11-09","08:00:00","18:12:24",612,552,60,Some(72));

// Modify:聚合用9个540分钟工作日和两个周末样本，避免发布本人历史打卡流水。
check!(synthetic_period_aggregation,{
    let data=[("2026-10-26","08:00:00","18:00:00"),("2026-10-27","08:00:00","18:00:00"),("2026-10-28","08:00:00","18:00:00"),
        ("2026-10-29","08:00:00","18:00:00"),("2026-10-30","08:00:00","18:00:00"),("2026-10-31","10:00:00","15:00:00"),
        ("2026-11-01","14:00:00","18:00:00"),("2026-11-02","08:00:00","18:00:00"),("2026-11-03","08:00:00","18:00:00"),
        ("2026-11-04","08:00:00","18:00:00"),("2026-11-05","08:00:00","18:00:00")].map(|(date,on,off)|day(date,on,off));
    let now=clock("2026-11-06 20:00:00");let summary=summarize(&period(now),&data,now,true);
    assert_eq!((summary.total,summary.weekday,summary.overtime,summary.weekend),(5400,4860,540,540));assert!(summary.reliable);assert_eq!(summary.period.working_days,22);
});
check!(floor_each_day_before_sum,{let now=clock("2026-11-06 20:00:00");let data=[day("2026-11-02","08:00:01","08:01:00"),day("2026-11-03","08:00:01","08:01:00")];assert_eq!(summarize(&period(now),&data,now,true).total,0);});
check!(lunch_pauses_live_clock,{let open=open_day("2026-11-06","08:00:00");for time in ["12:00:00","12:30:00","13:00:00"]{assert_eq!(calculate_day(&open,clock(&format!("2026-11-06 {time}"))).minutes,Some(240));}assert_eq!(calculate_day(&open,clock("2026-11-06 13:01:00")).minutes,Some(241));});
check!(real_off_duty_stops_clock,{let final_day=day("2026-11-06","08:30:00","18:40:59");assert_eq!(calculate_day(&final_day,clock("2026-11-06 20:00:00")).minutes,Some(550));assert_eq!(calculate_day(&final_day,clock("2026-11-06 23:00:00")).minutes,Some(550));});
check!(delayed_card_reduces_excess,{let open=open_day("2026-11-06","08:30:00");let closed=day("2026-11-06","08:30:00","18:00:00");assert_eq!(calculate_day(&open,clock("2026-11-06 20:00:00")).minutes,Some(630));let result=calculate_day(&closed,clock("2026-11-06 20:00:00"));assert_eq!((result.minutes,result.overtime),(Some(510),Some(30)));});
check!(late_lunch_off_duty_recalculates,{let open=open_day("2026-11-06","08:00:00");assert_eq!(calculate_day(&open,clock("2026-11-06 13:30:00")).minutes,Some(270));assert_eq!(calculate_day(&day("2026-11-06","08:00:00","12:30:00"),clock("2026-11-06 13:30:00")).minutes,Some(240));});
check!(same_day_live_counted,{let now=clock("2026-11-06 18:00:00");let summary=summarize(&period(now),&[open_day("2026-11-06","08:00:00")],now,true);assert_eq!((summary.total,summary.overtime),(540,60));assert!(summary.reliable);});
check!(future_and_short_days_do_not_subtract,{let now=clock("2026-11-06 20:00:00");let summary=summarize(&period(now),&[day("2026-11-02","08:00:00","20:00:00"),day("2026-11-03","08:00:00","14:00:00")],now,true);assert_eq!(summary.overtime,180);});
check!(weekend_never_adds_weekday_overtime,{let now=clock("2026-11-06 20:00:00");let summary=summarize(&period(now),&[day("2026-10-31","08:00:00","23:00:00")],now,true);assert_eq!((summary.overtime,summary.weekday),(0,0));assert_eq!(summary.total,780);});
check!(duplicate_day_snapshot_replaces,{let now=clock("2026-11-06 20:00:00");let summary=summarize(&period(now),&[day("2026-11-02","08:00:00","20:00:00"),day("2026-11-02","08:00:00","17:00:00")],now,true);assert_eq!(summary.total,480);});
check!(duplicate_same_punch_not_multi_shift,{let mut input=day("2026-11-02","08:00:00","17:00:00");input.records.push(input.records[0].clone());assert_eq!(calculate_day(&input,clock("2026-11-02 20:00:00")).state,DayState::Complete);});
check!(two_distinct_starts_require_review,{let mut input=day("2026-11-02","08:00:00","17:00:00");let mut second=input.records[0].clone();second.time+=1000;input.records.push(second);assert_eq!(calculate_day(&input,clock("2026-11-02 20:00:00")).state,DayState::NeedsReview);});
check!(past_missing_end_not_live,{assert_eq!(calculate_day(&open_day("2026-11-02","08:00:00"),clock("2026-11-03 08:00:00")).state,DayState::NeedsReview);});
check!(cross_midnight_not_auto_paired,{let mut input=day("2026-11-02","23:00:00","23:30:00");input.records[1].time=clock("2026-11-03 01:00:00").timestamp_millis();assert_eq!(calculate_day(&input,clock("2026-11-03 08:00:00")).state,DayState::NeedsReview);});
check!(reverse_times_require_review,{assert_eq!(calculate_day(&day("2026-11-02","17:00:00","08:00:00"),clock("2026-11-02 20:00:00")).state,DayState::NeedsReview);});
check!(future_timestamp_on_today_not_counted,{assert_eq!(calculate_day(&day("2026-11-02","08:00:00","20:00:00"),clock("2026-11-02 10:00:00")).state,DayState::NeedsReview);});
check!(future_date_not_counted,{assert_eq!(calculate_day(&day("2026-11-03","08:00:00","17:00:00"),clock("2026-11-02 20:00:00")).state,DayState::Future);});
check!(unknown_kind_requires_review,{let mut input=open_day("2026-11-02","08:00:00");input.records[0].kind=PunchKind::Unknown;assert_eq!(calculate_day(&input,clock("2026-11-02 20:00:00")).state,DayState::NeedsReview);});
check!(no_records_not_zero_minutes,{let input=empty_day(date("2026-11-02"));let result=calculate_day(&input,clock("2026-11-02 20:00:00"));assert_eq!((result.state,result.minutes),(DayState::NoRecords,None));});
check!(empty_review_flag_preserved,{let mut input=empty_day(date("2026-11-02"));input.review=true;assert_eq!(calculate_day(&input,clock("2026-11-02 20:00:00")).state,DayState::NeedsReview);});
check!(incomplete_data_blocks_rewards,{let now=clock("2026-11-06 20:00:00");let summary=summarize(&period(now),&[day("2026-11-02","08:00:00","17:00:00")],now,false);assert!(!summary.reliable);assert!(summary.has_data);});

check!(cycle_before_25,{let p=period(clock("2026-11-24 23:59:59"));assert_eq!((p.start,p.end),(date("2026-10-25"),date("2026-11-24")));});
check!(cycle_switch_at_25,{let p=period(clock("2026-11-25 00:00:00"));assert_eq!((p.start,p.end),(date("2026-11-25"),date("2026-12-24")));});
check!(cycle_crosses_year,{let p=period(clock("2027-01-01 00:00:00"));assert_eq!((p.start,p.end),(date("2026-12-25"),date("2027-01-24")));});
check!(cycle_leap_february,{let p=period(clock("2028-02-29 10:00:00"));assert_eq!((p.start,p.end),(date("2028-02-25"),date("2028-03-24")));});
check!(cycle_utc_boundary_uses_china,{let p=period(Utc.with_ymd_and_hms(2026,11,24,16,0,0).unwrap());assert_eq!(p.start,date("2026-11-25"));});
check!(cycle_full_weekday_baseline,{let p=period(clock("2026-11-05 08:00:00"));assert_eq!(p.working_days,22);assert_eq!(p.working_days*480,10560);});

macro_rules! reward {($name:ident,$category:expr,$level:expr,$current:expr,$reached:expr)=>{check!($name,{let s=synthetic($current,$current,true);let ts=treasures(&s,"self",&[]);let t=ts.iter().find(|t|t.category==$category&&t.level==$level).unwrap();assert_eq!(t.reached,$reached);});};}
reward!(total_250_equal_not_reached,Category::Total,1,15000,false);
reward!(total_250_plus_minute_reached,Category::Total,1,15001,true);
reward!(total_280_equal_not_reached,Category::Total,2,16800,false);
reward!(total_280_plus_minute_reached,Category::Total,2,16801,true);
reward!(total_320_equal_not_reached,Category::Total,3,19200,false);
reward!(total_320_plus_minute_reached,Category::Total,3,19201,true);
reward!(weekday_30_equal_reached,Category::Weekday,1,1800,true);
reward!(weekday_50_equal_reached,Category::Weekday,2,3000,true);
reward!(weekday_70_equal_reached,Category::Weekday,3,4200,true);
check!(reward_highest_category_sum_2500,{let ts=treasures(&synthetic(19201,4200,true),"self",&[]);let sum:i64=[Category::Total,Category::Weekday].iter().map(|category|ts.iter().filter(|t|t.category==*category&&t.reached).map(|t|t.award).max().unwrap_or(0)).sum();assert_eq!(sum,2500);});
check!(visible_first_until_claim,{let s=synthetic(19201,4200,true);let ts=treasures(&s,"self",&[]);assert_eq!(visible(&ts,Category::Total).unwrap().level,1);assert!(!ts.iter().find(|t|t.category==Category::Total&&t.level==2).unwrap().can_claim);});
check!(sequential_claim_and_no_double_claim,{let s=synthetic(19201,4200,true);let now=clock("2026-11-06 20:00:00");let mut claims=vec![];assert!(!claim(&mut claims,&s,"self",Category::Total,2,now));for category in [Category::Total,Category::Weekday]{for level in 1..=3{assert!(claim(&mut claims,&s,"self",category,level,now));assert!(!claim(&mut claims,&s,"self",category,level,now));}}assert_eq!(claims.len(),6);});
check!(unreliable_or_unreached_cannot_claim,{let now=clock("2026-11-06 20:00:00");assert!(!claim(&mut vec![],&synthetic(20000,5000,false),"self",Category::Total,1,now));assert!(!claim(&mut vec![],&synthetic(0,0,true),"self",Category::Weekday,1,now));});
check!(correction_preserves_claim_and_unlock,{let mut claims=vec![];let s=synthetic(15001,0,true);assert!(claim(&mut claims,&s,"self",Category::Total,1,clock("2026-11-06 20:00:00")));let ts=treasures(&synthetic(100,0,true),"self",&claims);assert!(ts[0].claimed);assert!(!ts[0].reached);assert_eq!(visible(&ts,Category::Total).unwrap().level,2);});
check!(claims_isolated_by_identity_and_period,{let p=period(clock("2026-11-06 20:00:00"));assert_ne!(claim_key("one",&p,Category::Total,1),claim_key("two",&p,Category::Total,1));assert_ne!(claim_key("one",&p,Category::Total,1),claim_key("one",&period(clock("2026-11-25 08:00:00")),Category::Total,1));});

check!(pace_weekdays_today_open_and_closed,{let p=period(clock("2026-11-06 20:00:00"));let now=clock("2026-11-09 16:00:00");assert_eq!(pace(&p,now,false,905,true),(12,76));assert_eq!(pace(&p,now,true,905,true),(11,83));});
check!(pace_calendar_today_open_and_closed,{let p=period(clock("2026-11-06 20:00:00"));let now=clock("2026-11-09 16:00:00");assert_eq!(pace(&p,now,false,7920,false),(16,495));assert_eq!(pace(&p,now,true,7920,false),(15,528));});
check!(pace_weekend_inclusion_differs,{let now=clock("2026-11-14 16:00:00");let p=period(now);assert_eq!(pace(&p,now,false,421,true),(7,61));assert_eq!(pace(&p,now,false,421,false),(11,39));});
check!(pace_period_end_no_division_by_zero,{let now=clock("2026-11-24 10:00:00");let p=period(now);assert_eq!(pace(&p,now,false,60,false),(1,60));assert_eq!(pace(&p,now,true,60,false),(0,0));assert_eq!(pace(&p,clock("2026-11-25 10:00:00"),false,60,false),(0,0));});
check!(pace_before_period_clamped,{let p=period(clock("2026-11-06 20:00:00"));assert_eq!(pace(&p,clock("2026-10-24 10:00:00"),false,60,false),(31,2));assert_eq!(pace(&p,clock("2026-10-24 10:00:00"),false,60,true).0,22);});
check!(pace_round_up_and_nonnegative,{let now=clock("2026-11-09 16:00:00");let p=period(now);for (remaining,expected) in [(1,1),(1600,100),(1601,101),(0,0),(-1,0)]{assert_eq!(pace(&p,now,false,remaining,false).1,expected);}});
check!(pace_next_tier_after_claim,{let s=synthetic(15001,1800,true);let mut claims=vec![];let now=clock("2026-11-09 16:00:00");assert!(claim(&mut claims,&s,"self",Category::Total,1,now));let ts=treasures(&s,"self",&claims);let target=visible(&ts,Category::Total).unwrap();assert_eq!(pace(&s.period,now,false,target.required-target.current,false),(16,113));});

check!(personal_valid_on_duty,{let (_,_,day)=personal(&personal_response(json!([card(json!("上班"),true)])),date("2026-11-09")).unwrap();assert!(!day.review);assert_eq!(day.records[0].kind,PunchKind::OnDuty);});
check!(personal_ignores_zero_plan_card,{let response=personal_response(json!([{"userCheckTime":0,"checkTypeDesc":"下班"}]));assert!(personal(&response,date("2026-11-09")).unwrap().2.records.is_empty());});
check!(personal_missing_actual_time_requires_review,{let response=personal_response(json!([{"checkTypeDesc":"上班"}]));assert!(personal(&response,date("2026-11-09")).unwrap().2.review);});
check!(personal_missing_identity_rejected,{let mut response=personal_response(json!([]));response["result"]["currentUserId"]=Value::Null;assert!(personal(&response,date("2026-11-09")).is_err());});
check!(personal_missing_array_rejected,{assert!(personal(&json!({"currentUserId":"self"}),date("2026-11-09")).is_err());});
check!(business_failure_not_empty_data,{assert!(payload(&json!({"success":false,"result":[]})).is_err());});
check!(null_payload_rejected,{assert!(payload(&json!({"success":true,"result":null})).is_err());});
check!(numeric_user_id_supported,{let response=json!({"currentUserId":123,"recordList":[]});assert_eq!(personal(&response,date("2026-11-09")).unwrap().0,"123");});
check!(invalid_duplicate_same_millisecond_excluded,{let response=personal_response(json!([card(Value::Null,false),card(json!("上班"),true)]));let day=personal(&response,date("2026-11-09")).unwrap().2;assert!(!day.review);assert_eq!(day.records.len(),1);});
check!(invalid_duplicate_different_millisecond_review,{let mut bad=card(Value::Null,false);bad["userCheckTime"]=json!(bad["userCheckTime"].as_i64().unwrap()+1);let day=personal(&personal_response(json!([bad,card(json!("上班"),true)])),date("2026-11-09")).unwrap().2;assert!(day.review);});
check!(unknown_normal_item_not_discarded,{let day=personal(&personal_response(json!([card(Value::Null,true),card(json!("上班"),true)])),date("2026-11-09")).unwrap().2;assert!(day.review);});
check!(typed_abnormal_card_retained,{let day=personal(&personal_response(json!([card(json!("上班"),false)])),date("2026-11-09")).unwrap().2;assert!(!day.review);assert_eq!(day.records.len(),1);assert!(!day.note.is_empty());});
check!(unknown_nonempty_description_review,{let day=personal(&personal_response(json!([card(json!("其他"),false),card(json!("上班"),true)])),date("2026-11-09")).unwrap().2;assert!(day.review);});
check!(batch_unique_nested_array,{let (raw,_)=batch();assert_eq!(records(&json!({"result":{"records":raw}})).unwrap().len(),1);});
check!(batch_ambiguous_arrays_rejected,{let (raw,_)=batch();assert!(records(&json!({"one":raw,"two":raw})).is_err());});
check!(batch_unknown_shape_rejected,{assert!(records(&json!({"unexpected":"value"})).is_err());});
check!(batch_one_empty_array_is_valid,{assert!(records(&json!({"records":[]})).unwrap().is_empty());});
check!(batch_has_more_nested,{assert!(has_more(&json!({"result":{"hasMore":true}})));assert!(has_more(&json!({"has_more":true})));assert!(!has_more(&json!({"hasMore":false})));});
check!(batch_valid_record_pairing,{let (raw,results)=batch();let days=build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).unwrap();assert_eq!(days.len(),1);assert_eq!(days[0].records.len(),1);});
check!(batch_extra_raw_not_counted,{let (mut raw,results)=batch();let mut extra=raw[0].clone();extra["id"]=json!("extra");raw.push(extra);assert_eq!(build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).unwrap()[0].records.len(),1);});
check!(batch_user_mismatch_rejected,{let (mut raw,results)=batch();raw[0]["userId"]=json!("other");assert!(build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).is_err());});
check!(batch_result_user_mismatch_rejected,{let (raw,mut results)=batch();results[0]["userId"]=json!("other");assert!(build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).is_err());});
check!(batch_raw_missing_id_rejected,{let (mut raw,results)=batch();raw[0]["id"]=Value::Null;assert!(build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).is_err());});
check!(batch_timestamp_mismatch_rejected,{let (mut raw,results)=batch();raw[0]["userCheckTime"]=json!(1);assert!(build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).is_err());});
check!(batch_date_outside_range_rejected,{let (raw,results)=batch();assert!(build_days("self",date("2026-11-03"),date("2026-11-03"),&results,&raw).is_err());});
check!(batch_missing_work_date_rejected,{let (raw,mut results)=batch();results[0]["workDate"]=Value::Null;assert!(build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).is_err());});
check!(batch_missing_actual_time_not_empty,{let (raw,mut results)=batch();results[0]["userCheckTime"]=Value::Null;let days=build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).unwrap();assert!(days[0].review);});
check!(batch_absence_with_time_requires_review,{let (raw,mut results)=batch();results[0]["timeResult"]=json!("NotSigned");assert!(build_days("self",date("2026-11-02"),date("2026-11-02"),&results,&raw).unwrap()[0].review);});
check!(decode_structured_content,{assert_eq!(decode(&json!({"structuredContent":{"ok":true}})).unwrap(),json!({"ok":true}));});
check!(decode_single_json_text,{assert_eq!(decode(&json!({"content":[{"type":"text","text":"{\"ok\":true}"}]})).unwrap(),json!({"ok":true}));});
check!(decode_tool_error_rejected,{assert!(decode(&json!({"isError":true,"structuredContent":[]})).is_err());});
check!(decode_ambiguous_text_rejected,{assert!(decode(&json!({"content":[{"type":"text","text":"{}"},{"type":"text","text":"{}"}]})).is_err());});
check!(decode_non_json_rejected,{assert!(decode(&json!({"content":[{"type":"text","text":"error"}]})).is_err());});
check!(schema_all_verified_tools,{for index in 0..3{validate_schema(TOOLS[index],&schema(index)).unwrap();}});
check!(schema_missing_required_field_rejected,{let mut input=schema(0);input["properties"]=json!({});assert!(validate_schema(TOOLS[0],&input).is_err());});
check!(schema_unknown_required_rejected,{let mut input=schema(0);input["required"]=json!(["workDate","newField"]);assert!(validate_schema(TOOLS[0],&input).is_err());});
check!(schema_integer_timestamp_allowed,{let mut input=schema(0);input["properties"]["workDate"]["type"]=json!("integer");assert!(validate_schema(TOOLS[0],&input).is_ok());});
check!(schema_wrong_wrapper_type_rejected,{let mut input=schema(1);input["properties"]["QueryCheckRecordRequest"]["type"]=json!("string");assert!(validate_schema(TOOLS[1],&input).is_err());});

check!(dpapi_roundtrip_and_not_plaintext,{let text=b"isolated-test-token";let encrypted=protect(text,true).unwrap();assert_ne!(encrypted,text);assert!(!encrypted.windows(text.len()).any(|p|p==text));assert_eq!(protect(&encrypted,false).unwrap(),text);});
check!(dpapi_corruption_rejected,{assert!(protect(b"not-a-dpapi-blob",false).is_err());});
check!(settings_default_isolated_new_application,{let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();let settings=store.settings().unwrap();assert_eq!(settings.opacity,1.);assert!(settings.topmost);assert!(!store.has_tokens());});
check!(settings_roundtrip_restart,{let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();store.write("settings.json",&Settings{opacity:0.55,topmost:false,floating_position:Some((200.,100.))}).unwrap();let restored=Store::open(Some(directory.path().into())).unwrap().settings().unwrap();assert_eq!(restored.opacity,0.55);assert!(!restored.topmost);assert_eq!(restored.floating_position,Some((200.,100.)));});
check!(settings_out_of_range_clamped,{let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();store.write("settings.json",&json!({"opacity":0.01})).unwrap();assert_eq!(store.settings().unwrap().opacity,0.2);});
check!(settings_legacy_missing_defaults,{let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();store.write("settings.json",&json!({})).unwrap();assert_eq!(store.settings().unwrap().opacity,1.);});
check!(connection_session_persists,{let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();let connection=Connection::default();store.write("connection.json",&connection).unwrap();assert_eq!(Store::open(Some(directory.path().into())).unwrap().connection().unwrap().session,connection.session);});
check!(claims_survive_restart_and_keep_idempotence,{let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();let now=clock("2026-11-06 20:00:00");let s=synthetic(15001,0,true);let mut claims=vec![];assert!(claim(&mut claims,&s,"self",Category::Total,1,now));store.write("claims.json",&claims).unwrap();let mut restored=Store::open(Some(directory.path().into())).unwrap().claims().unwrap();assert!(!claim(&mut restored,&s,"self",Category::Total,1,now));});
check!(claims_corrupt_not_silently_cleared,{let directory=tempfile::tempdir().unwrap();std::fs::write(directory.path().join("claims.json"),"invalid").unwrap();assert!(Store::open(Some(directory.path().into())).unwrap().claims().is_err());});
check!(cache_identity_period_isolation,{let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();let now=clock("2026-11-06 20:00:00");let p=period(now);let mut cache=Cache{identity:"self".into(),user_id:"self".into(),group:"test".into(),period:p.clone(),days:vec![day("2026-11-02","08:00:00","17:00:00")],last_sync:now,complete:true};store.save_cache(&cache).unwrap();cache.days[0]=day("2026-11-02","08:00:00","18:00:00");store.save_cache(&cache).unwrap();assert_eq!(store.cache("self",&p).unwrap().unwrap().days.len(),1);assert!(store.cache("other",&p).unwrap().is_none());assert!(store.cache("self",&period(now+Duration::days(31))).unwrap().is_none());});
