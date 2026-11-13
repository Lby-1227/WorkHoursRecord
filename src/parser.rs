// Modify:移植已验证的官方考勤字段、异常重复卡及流水关联校验。$saving-ai-agent-tokens
use crate::domain::*;
use anyhow::{bail, ensure, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// 功能：提取字符串或整数ID；输入JSON与字段名，输出字符串或空值。
pub fn text(value: &Value, key: &str) -> String {
    match &value[key] { Value::String(value) => value.clone(), Value::Number(value) => value.to_string(), _ => String::new() }
}
/// 功能：提取整数时间或ID；输入JSON及字段名，输出整数或缺失，绝不将无效值置零。
pub fn number(value: &Value, key: &str) -> Option<i64> { text(value, key).parse().ok() }
/// 功能：校验业务回执包装；输入JSON，输出实际载荷引用或明确错误。
pub fn payload(mut value: &Value) -> Result<&Value> {
    for _ in 0..8 {
        ensure!(value.get("success").is_none_or(|s| s == true), "钉钉未返回成功结果，请核对权限及授权。");
        match value.get("result") { Some(result) => { ensure!(!result.is_null(), "考勤返回为空，不能确认完整性。"); value = result; }, None => return Ok(value) }
    }
    bail!("考勤结果包装层级异常。")
}
/// 功能：校验有效毫秒时间戳；输入整数，输出UTC时刻或错误。
pub fn timestamp(value: i64) -> Result<chrono::DateTime<Utc>> {
    Utc.timestamp_millis_opt(value).single().ok_or_else(|| anyhow::anyhow!("考勤时间不是有效毫秒时间戳。"))
}
/// 功能：解析本人单日记录；输入载荷及日期，输出本人ID、组名和日快照。
pub fn personal(response: &Value, date: NaiveDate) -> Result<(String, String, AttendanceDay)> {
    let value = payload(response)?; let user = text(value, "currentUserId");
    let list = value["recordList"].as_array().ok_or_else(|| anyhow::anyhow!("个人考勤缺少recordList。"))?;
    ensure!(!user.is_empty(), "个人考勤缺少currentUserId。");
    let mut day = empty_day(date); let mut candidates = Vec::new(); let mut abnormal = false;
    // Modify:无类型异常项必须有同毫秒可识别卡佐证才排除，其他未知项保留待核对。
    for item in list {
        let Some(time) = number(item, "userCheckTime").filter(|time| *time >= 0) else { day.review = true; continue; };
        if time == 0 { continue; } timestamp(time)?;
        let description = text(item, "checkTypeDesc"); let bad = item["isNormal"] == false;
        let kind = if description == "OnDuty" || description.contains("上班") { PunchKind::OnDuty }
            else if description == "OffDuty" || description.contains("下班") { PunchKind::OffDuty } else { PunchKind::Unknown };
        let punch = Punch { id: format!("personal:{time}:{kind:?}"), kind, time };
        if kind == PunchKind::Unknown && description.trim().is_empty() && bad { candidates.push(punch); continue; }
        day.review |= kind == PunchKind::Unknown; abnormal |= bad; day.records.push(punch);
    }
    let typed: HashSet<_> = day.records.iter().filter(|p| p.kind != PunchKind::Unknown).map(|p| p.time).collect();
    let mut removed = false;
    for punch in candidates { if typed.contains(&punch.time) { removed = true; } else { day.records.push(punch); day.review = true; } }
    day.note = if day.review { "个人打卡字段缺失或类型无法唯一识别" } else if abnormal { "含钉钉标记的异常打卡" }
        else if removed { "已排除同一时刻的无效重复打卡" } else { "" }.into();
    Ok((user, text(value, "groupName"), day))
}
/// 功能：判别实际流水成员；输入节点，输出是否具有已核对的记录字段。
fn is_record(value: &Value) -> bool { value.is_object() && value.get("userId").is_some() && (value.get("userCheckTime").is_some() || value.get("checkType").is_some()) }
/// 功能：收集唯一记录数组；输入节点、深度及候选集合，输出通过集合传递。
fn arrays<'a>(value: &'a Value, depth: usize, candidates: &mut Vec<&'a Vec<Value>>) {
    if depth > 8 { return; }
    if let Some(object) = value.as_object() {
        for node in object.values() {
            if let Some(array) = node.as_array() { if array.iter().all(is_record) { candidates.push(array); } }
            else if node.is_object() { arrays(node, depth + 1, candidates); }
        }
    }
}
/// 功能：解析批量唯一数组；输入JSON，输出独立记录集合，结构不明确时失败。
pub fn records(response: &Value) -> Result<Vec<Value>> {
    let value = payload(response)?;
    if let Some(array) = value.as_array() { ensure!(array.iter().all(is_record), "批量数组成员字段不匹配。"); return Ok(array.clone()); }
    let mut candidates = vec![]; arrays(value, 0, &mut candidates);
    let populated: Vec<_> = candidates.iter().filter(|array| !array.is_empty()).collect();
    let selected = if populated.len() == 1 { *populated[0] } else if populated.is_empty() && candidates.len() == 1 { candidates[0] }
        else { bail!("批量记录数组不唯一或结构未知。"); };
    Ok(selected.clone())
}
/// 功能：检查未完成分页标志；输入响应节点，输出是否明确还有记录。
pub fn has_more(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.iter().any(|(key, node)|
        ((key == "hasMore" || key == "has_more") && node == true) || (node.is_object() && has_more(node))))
}
/// 功能：核对本人身份；输入JSON及本人ID，输出成功或阻止合并的错误。
fn verify_user(value: &Value, user: &str) -> Result<()> { ensure!(text(value, "userId") == user, "返回记录userId与本人不一致。"); Ok(()) }
/// 功能：关联完整结果和流水；输入身份、日期区间及两组记录，输出未经多段拼接的按日快照。
pub fn build_days(user: &str, from: NaiveDate, to: NaiveDate, results: &[Value], raw_records: &[Value]) -> Result<Vec<AttendanceDay>> {
    let mut raw = HashMap::new();
    for record in raw_records {
        verify_user(record, user)?; let id = text(record, "id"); ensure!(!id.is_empty(), "打卡流水缺少id。");
        if let Some(old) = raw.insert(id, record) { ensure!(old == record, "同一流水返回冲突内容。"); }
    }
    let mut days = BTreeMap::new();
    // Modify:只计入判定明确关联的流水；额外无效尝试不增加实际工时。
    for record in results {
        verify_user(record, user)?;
        let work = number(record, "workDate").ok_or_else(|| anyhow::anyhow!("打卡结果缺少归属日。"))?;
        let date = today(timestamp(work)?); ensure!(date >= from && date <= to, "打卡结果超出查询日期范围。");
        let day = days.entry(date).or_insert_with(|| empty_day(date));
        let Some(time) = number(record, "userCheckTime").filter(|time| *time >= 0) else { day.review = true; day.note = "实际打卡时间字段缺失或无效".into(); continue; };
        let state = text(record, "timeResult");
        if time == 0 { if !state.is_empty() && state != "Normal" { day.note = format!("考勤判定：{state}"); } continue; }
        timestamp(time)?;
        if state == "NotSigned" || state == "Absenteeism" { day.review = true; continue; }
        let id = text(record, "recordId");
        ensure!(!id.is_empty() && raw.get(&id).is_some_and(|actual| number(actual, "userCheckTime") == Some(time)), "打卡结果与流水不完整或时间不一致。");
        let kind = match text(record, "checkType").as_str() { "OnDuty" => PunchKind::OnDuty, "OffDuty" => PunchKind::OffDuty, _ => PunchKind::Unknown };
        day.review |= kind == PunchKind::Unknown;
        if !state.is_empty() && state != "Normal" { day.note = format!("考勤判定：{state}"); }
        day.records.push(Punch { id, kind, time });
    }
    Ok(days.into_values().collect())
}
