// Modify:官方OAuth与三个只读MCP工具，独立授权、分页核验和冷启动刷新。$saving-ai-agent-tokens
use crate::{domain::*, parser, store::{Store, Vault, Registration, ENDPOINT}};
use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rmcp::{ServiceExt, RoleClient, service::RunningService, model::CallToolRequestParams,
    transport::{StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    auth::{AuthClient, AuthorizationManager, AuthorizationRequest, OAuthClientConfig, OAuthState, default_oauth_http_client}}};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::Arc};
use tokio::{net::TcpListener, io::{AsyncReadExt, AsyncWriteExt}};

pub const TOOLS: [&str; 3] = ["get_personal_attendance_records", "query_check_record", "query_check_result"];
pub struct AttendanceClient { service: RunningService<RoleClient, ()> }

impl AttendanceClient {
    /// 功能：连接固定官方服务；输入本应用存储和是否交互，输出已核验工具Schema的客户端。
    pub async fn connect(store: Arc<Store>, interactive: bool) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
        // Modify:先在10秒内确认官方网关可建立TLS，避免网络故障被误显示为等待本人授权5分钟。
        let probe = tokio::time::timeout(std::time::Duration::from_secs(10), http.get(ENDPOINT).send()).await;
        ensure!(matches!(probe,Ok(Ok(_))), "无法连接钉钉官方网关，请先检查网络或代理能否访问钉钉官方网页，再重试连接。");
        // Modify:使用SDK公开的网络适配入口，兼容本机代理虚拟DNS；仍固定官方HTTPS端点并保留TLS校验。
        let oauth_http = default_oauth_http_client().map_err(|_| anyhow::anyhow!("官方OAuth网络初始化失败。"))?;
        let mut manager = AuthorizationManager::new_with_oauth_http_client(ENDPOINT, Arc::new(oauth_http)).await
            .map_err(|_| anyhow::anyhow!("官方OAuth初始化失败，请检查网络。"))?;
        let discovery = manager.resolve_metadata().await.map_err(|_| anyhow::anyhow!("官方OAuth服务发现未完成。"))?;
        ensure!(discovery.source.is_discovered(), "官方OAuth未提供发现信息，未推断授权地址。");
        let metadata = discovery.metadata;
        manager.set_metadata(metadata.clone()); manager.set_credential_store(Vault(store.clone()));
        let manager = if interactive {
            let listener = TcpListener::bind("127.0.0.1:0").await.context("无法创建本机授权回调。")?;
            let redirect = format!("http://127.0.0.1:{}/oauth/callback", listener.local_addr()?.port());
            let registration_url = metadata.registration_endpoint.clone().context("官方服务不支持动态客户端注册。")?;
            let registered = register(&http,&registration_url,&redirect,metadata.issuer.clone().unwrap_or_default()).await?;
            let client_id=registered.client_id.clone();let client_secret=registered.client_secret.clone();
            store.save_registration(registered)?;
            let mut oauth = OAuthState::Unauthorized(manager);
            let mut request = AuthorizationRequest::new(&redirect).with_client_name("Work Hours Record Rust")
                .with_application_type("native").with_scopes(["user"]).with_preregistered_client(client_id);
            if let Some(secret) = client_secret { request = request.with_client_secret(secret); }
            oauth.start_authorization(request).await.map_err(|_| anyhow::anyhow!("官方OAuth授权准备失败。"))?;
            let url = oauth.get_authorization_url().await.map_err(|_| anyhow::anyhow!("官方OAuth未生成授权页面。"))?;
            open_url(&url)?;
            // Modify:仅回环地址接收回调；SDK负责PKCE、state及存在时的issuer核验，错误不输出URL或令牌。
            loop {
                let (mut stream, _) = listener.accept().await?;
                let mut buffer = vec![0u8; 8192]; let mut used = 0;
                while used < buffer.len() {
                    let count = stream.read(&mut buffer[used..]).await?; if count == 0 { break; } used += count;
                    if buffer[..used].windows(4).any(|part| part == b"\r\n\r\n") { break; }
                }
                let request = String::from_utf8_lossy(&buffer[..used]); let mut parts = request.lines().next().unwrap_or("").split_whitespace();
                let method = parts.next().unwrap_or(""); let path = parts.next().unwrap_or("");
                let callback = reqwest::Url::parse(&format!("http://127.0.0.1{path}"));
                if method != "GET" || !callback.as_ref().is_ok_and(|url| url.path() == "/oauth/callback") {
                    stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?; continue;
                }
                let callback = callback?;
                let outcome = oauth.handle_callback_url(callback.as_str()).await;
                let body = if outcome.is_ok() { "授权完成，可以关闭网页并返回工时记录。" } else { "授权未完成，请返回工时记录重新连接。" };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await?;
                outcome.map_err(|_| anyhow::anyhow!("OAuth回调验证或令牌交换失败，请重新连接。"))?;
                break;
            }
            oauth.into_authorization_manager().context("官方授权未进入完成状态。")?
        } else {
            let registration = store.registration().context("需要本人在设置中连接钉钉。")?;
            ensure!(registration.issuer == metadata.issuer.clone().unwrap_or_default(), "授权服务身份变化，请重新连接。");
            ensure!(manager.initialize_from_store().await.map_err(|_| anyhow::anyhow!("需要重新授权，请在设置中连接。"))?, "需要本人在设置中连接钉钉。");
            // Modify:SDK恢复客户端ID后重新配置已加密保存的secret，确保冷启动刷新仍采用服务端声明的post方式。
            let mut config = OAuthClientConfig::new(registration.client_id, registration.redirect).with_scopes(vec!["user".into()]);
            if let Some(secret) = registration.client_secret { config = config.with_client_secret(secret); }
            manager.configure_client(config).map_err(|_| anyhow::anyhow!("本应用客户端配置无法恢复。"))?;
            manager
        };
        let authorized = AuthClient::new(http, manager);
        let transport = StreamableHttpClientTransport::with_client(authorized, StreamableHttpClientTransportConfig::with_uri(ENDPOINT));
        let service = ().serve(transport).await.map_err(|_| anyhow::anyhow!("官方MCP连接未完成，需要重试或重新授权。"))?;
        let tools = service.list_all_tools().await.map_err(|_| anyhow::anyhow!("读取官方考勤工具目录失败。"))?;
        for name in TOOLS {
            let tool = tools.iter().find(|tool| tool.name.as_ref() == name).with_context(|| format!("考勤服务未提供{name}。"))?;
            validate_schema(name, &Value::Object((*tool.input_schema).clone()))?;
        }
        Ok(Self { service })
    }
    /// 功能：执行白名单读取；输入工具名及JSON参数，输出唯一JSON，拒绝工具错误或未知载荷。
    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value> {
        ensure!(TOOLS.contains(&name), "不允许调用非考勤读取工具。");
        let result = self.service.call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(arguments.as_object().cloned().context("读取参数必须是对象。")?))
            .await.map_err(|_| anyhow::anyhow!("官方考勤工具请求失败，请检查权限或网络。"))?;
        decode(&serde_json::to_value(result)?)
    }
    /// 功能：读取本人单日；输入北京时间日期，输出身份、组名及真实打卡快照。
    pub async fn personal(&self, date: NaiveDate) -> Result<(String, String, AttendanceDay)> {
        let midnight = china().from_local_datetime(&date.and_hms_opt(0,0,0).unwrap()).single().unwrap().timestamp_millis();
        parser::personal(&self.call(TOOLS[0], json!({"workDate":midnight})).await?, date)
    }
    /// 功能：分七天完整读取周期；输入周期、当前时刻和授权分区，输出已核对快照。
    pub async fn read_period(&self, period: &Period, now: DateTime<Utc>, session: &str) -> Result<Cache> {
        let date = today(now); let (user, _, _) = self.personal(date).await?;
        let mut from = period.start; let mut days = Vec::new();
        // Modify:流水明确无后续且判定分页读到空页后，才允许提交对应日期快照。
        while from <= period.end.min(date) {
            let to = (from + Duration::days(6)).min(period.end).min(date);
            let raw_response = self.call(TOOLS[1], json!({"QueryCheckRecordRequest":{"userIds":[user],"checkDateFrom":format!("{from} 00:00:00"),"checkDateTo":format!("{to} 23:59:59")}})).await?;
            ensure!(!parser::has_more(&raw_response), "打卡流水仍有下一页，但工具未提供分页参数。");
            let raw = parser::records(&raw_response)?;
            let mut results = Vec::new(); let mut ids = HashSet::new(); let mut offset = 0; let mut complete = false;
            for _ in 0..100 {
                let response = self.call(TOOLS[2], json!({"QueryCheckResultRequest":{"userIds":[user],"workDateFrom":format!("{from} 00:00:00"),"workDateTo":format!("{to} 23:59:59"),"offset":offset,"limit":100}})).await?;
                let records = parser::records(&response)?;
                if records.is_empty() { ensure!(!parser::has_more(&response), "考勤分页为空但仍有后续数据。"); complete = true; break; }
                for record in &records { let id = parser::text(record,"id"); ensure!(!id.is_empty() && ids.insert(id), "考勤分页缺少ID或重复返回旧页。"); }
                offset += records.len(); results.extend(records);
            }
            ensure!(complete, "考勤分页未读完，本轮不替换缓存。");
            days.extend(parser::build_days(&user, from, to, &results, &raw)?); from = to + Duration::days(1);
        }
        let (latest_user, group, latest) = self.personal(date).await?;
        ensure!(latest_user == user, "同步中本人身份变化，已停止合并。");
        days.retain(|day| day.date != date); days.push(latest);
        Ok(Cache { identity: format!("{session}:{user}"), user_id: user, group, period: period.clone(), days, last_sync: Utc::now(), complete: true })
    }
}
/// 功能：按官方发现信息动态注册本应用；输入HTTP客户端、注册地址、回调及发行方，输出需加密保存的注册对象。
async fn register(http:&reqwest::Client,url:&str,redirect:&str,issuer:String)->Result<Registration> {
    let uri=reqwest::Url::parse(url)?;
    // Modify:发布程序仅允许HTTPS；测试构建仅额外允许回环HTTP隔离服务器，不改变发布版入口。
    let loopback_test=cfg!(test)&&uri.scheme()=="http"&&uri.host_str()==Some("127.0.0.1");
    ensure!((uri.scheme()=="https"||loopback_test)&&uri.username().is_empty()&&uri.password().is_none(),"注册地址不是官方发现的有效HTTPS地址。");
    let registration:Value=http.post(uri).json(&json!({"client_name":"Work Hours Record Rust","application_type":"native","redirect_uris":[redirect],
        "grant_types":["authorization_code","refresh_token"],"response_types":["code"],"token_endpoint_auth_method":"client_secret_post"}))
        .send().await.map_err(|_|anyhow::anyhow!("官方客户端注册请求失败。"))?.error_for_status().map_err(|_|anyhow::anyhow!("官方客户端注册未获允许，请检查服务配置。"))?
        .json().await.map_err(|_|anyhow::anyhow!("官方客户端注册响应无效。"))?;
    let client_id=parser::text(&registration,"client_id");ensure!(!client_id.is_empty(),"官方注册缺少客户端标识。");
    Ok(Registration{client_id,client_secret:registration["client_secret"].as_str().map(String::from),redirect:redirect.into(),issuer})
}
/// 功能：校验已核对的工具Schema；输入工具名及实际Schema，输出成功或阻止请求的错误。
pub fn validate_schema(name: &str, schema: &Value) -> Result<()> {
    let properties = schema["properties"].as_object().context("考勤工具缺少参数Schema。")?;
    let (node, required, types): (&Value, &[&str], &[&str]) = match name {
        "get_personal_attendance_records" => (schema, &["workDate"], &["number"]),
        "query_check_record" => { check_required(schema, &["QueryCheckRecordRequest"])?; ensure!(properties["QueryCheckRecordRequest"]["type"] == "object", "考勤参数类型变化。"); (&properties["QueryCheckRecordRequest"], &["userIds","checkDateFrom","checkDateTo"], &["array","string","string"]) },
        "query_check_result" => { check_required(schema, &["QueryCheckResultRequest"])?; ensure!(properties["QueryCheckResultRequest"]["type"] == "object", "考勤参数类型变化。"); (&properties["QueryCheckResultRequest"], &["userIds","workDateFrom","workDateTo","offset","limit"], &["array","string","string","number","number"]) },
        _ => bail!("未知考勤工具。")
    };
    for (key, expected) in required.iter().zip(types) {
        let kind = node["properties"][key]["type"].as_str().unwrap_or("");
        ensure!(kind == *expected || (*expected == "number" && kind == "integer"), "考勤工具参数与已核对契约不一致。");
    }
    check_required(node, required)
}
/// 功能：禁止新增未知必填参数；输入Schema及本程序提供的字段，输出校验结果。
fn check_required(schema: &Value, supplied: &[&str]) -> Result<()> {
    if let Some(required) = schema.get("required") {
        ensure!(required.as_array().is_some_and(|items| items.iter().all(|item| item.as_str().is_some_and(|key| supplied.contains(&key)))), "考勤工具增加了未支持的必填参数。");
    } Ok(())
}
/// 功能：提取标准MCP回执；输入序列化回执，输出结构化或唯一文本JSON。
pub fn decode(result: &Value) -> Result<Value> {
    ensure!(result["isError"] != true, "考勤MCP工具执行失败。");
    if let Some(value) = result.get("structuredContent").filter(|value| !value.is_null()) { return Ok(value.clone()); }
    let texts: Vec<_> = result["content"].as_array().context("考勤回执缺少内容。")?.iter()
        .filter(|block| block["type"] == "text").filter_map(|block| block["text"].as_str()).filter(|text| !text.trim().is_empty()).collect();
    ensure!(texts.len() == 1, "考勤MCP未返回唯一JSON载荷。");
    serde_json::from_str(texts[0]).context("考勤MCP返回的不是JSON。")
}
/// 功能：交给系统浏览器打开网页；输入HTTP(S)地址，输出启动结果，不经Shell拼接命令。
pub fn open_url(url: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let parsed = reqwest::Url::parse(url)?; ensure!(parsed.scheme() == "https" || parsed.scheme() == "http", "网页地址无效。");
    std::process::Command::new("rundll32.exe").arg("url.dll,FileProtocolHandler").arg(url).creation_flags(0x08000000).spawn().context("无法打开系统浏览器。")?; Ok(())
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use axum::{Router, Json, extract::State, response::{IntoResponse,Response}, http::StatusCode, routing::post};
    use std::sync::atomic::{AtomicUsize,Ordering};
    struct Fixture { scenario:u8, personal_calls:AtomicUsize }

    /// 功能：提供隔离MCP服务器；输入场景和JSON-RPC请求，输出标准MCP JSON响应，不接触真实凭证。
    async fn respond(State(state):State<Arc<Fixture>>,Json(request):Json<Value>)->Response {
        let id=request.get("id").cloned();let method=request["method"].as_str().unwrap_or("");
        if id.is_none(){return StatusCode::ACCEPTED.into_response();}
        let result=match method {
            "initialize"=>json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"attendance-fixture","version":"1"}}),
            "tools/list"=>{
                let mut tools=Vec::new();
                for (index,name) in TOOLS.iter().enumerate(){
                    let mut schema=fixture_schema(index);if state.scenario==5&&index==0{schema["properties"]["workDate"]["type"]=json!("string");}
                    tools.push(json!({"name":name,"inputSchema":schema}));
                }json!({"tools":tools})
            },
            "tools/call"=>{
                let name=request["params"]["name"].as_str().unwrap_or("");let args=&request["params"]["arguments"];
                if state.scenario==9{return Json(json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":"denied"}]}})).into_response();}
                let payload=if name==TOOLS[0]{
                    let count=state.personal_calls.fetch_add(1,Ordering::SeqCst);let mut records=vec![json!({"checkTypeDesc":"上班","userCheckTime":millis("2026-11-09 08:00:00"),"isNormal":true})];
                    if state.scenario==10{records.push(json!({"checkTypeDesc":null,"userCheckTime":millis("2026-11-09 08:00:00"),"isNormal":false}));}
                    else if state.scenario!=8||count>0{records.push(json!({"checkTypeDesc":"下班","userCheckTime":millis("2026-11-09 19:00:00"),"isNormal":true}));}
                    json!({"currentUserId":if state.scenario==7&&count>0{"other"}else{"self"},"groupName":"fixture","recordList":records})
                }else{
                    let input=&args[if name==TOOLS[1]{"QueryCheckRecordRequest"}else{"QueryCheckResultRequest"}];
                    let from=input[if name==TOOLS[1]{"checkDateFrom"}else{"workDateFrom"}].as_str().unwrap_or("");
                    let to=input[if name==TOOLS[1]{"checkDateTo"}else{"workDateTo"}].as_str().unwrap_or("");
                    let included=from<="2026-11-02 00:00:00"&&to>="2026-11-02 23:59:59";
                    let mut records=vec![];
                    if included&&(name==TOOLS[1]||input["offset"]==0||state.scenario==1){
                        for (index,kind,time) in [(0,"OnDuty","2026-11-02 08:00:00"),(1,"OffDuty","2026-11-02 19:00:00")]{
                            records.push(if name==TOOLS[1]{json!({"id":format!("raw{index}"),"userId":if state.scenario==3{"other"}else{"self"},"userCheckTime":millis(time)+if state.scenario==6{1000}else{0},"checkType":kind})}
                                else{json!({"id":if state.scenario==11{String::new()}else{format!("result{index}")},"recordId":format!("raw{index}"),"userId":"self","userCheckTime":millis(time),"workDate":millis("2026-11-02 00:00:00"),"checkType":kind,"timeResult":"Normal"})});
                        }
                    }
                    if state.scenario==4{json!({"unexpected":true})}
                    else{json!({"records":records,"hasMore":(state.scenario==2&&name==TOOLS[2])||(state.scenario==12&&name==TOOLS[1])})}
                };
                json!({"content":[{"type":"text","text":payload.to_string()}]})
            },
            _=>json!({})
        };
        Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
    }
    /// 功能：生成单个工具的真实参数结构；输入工具下标，输出隔离Schema。
    fn fixture_schema(index:usize)->Value {
        if index==0{return json!({"type":"object","properties":{"workDate":{"type":"number"}},"required":["workDate"]});}
        let wrapper=if index==1{"QueryCheckRecordRequest"}else{"QueryCheckResultRequest"};
        let names:Vec<_>=if index==1{vec!["userIds","checkDateFrom","checkDateTo"]}else{vec!["userIds","workDateFrom","workDateTo","offset","limit"]};
        let mut props=serde_json::Map::new();for name in &names{props.insert((*name).into(),json!({"type":if *name=="userIds"{"array"}else if *name=="offset"||*name=="limit"{"number"}else{"string"}}));}
        json!({"type":"object","properties":{wrapper:{"type":"object","properties":props,"required":names}},"required":[wrapper]})
    }
    /// 功能：解析测试时间；输入北京时间文本，输出毫秒。
    fn millis(text:&str)->i64 {DateTime::parse_from_str(&format!("{text} +0800"),"%Y-%m-%d %H:%M:%S %z").unwrap().timestamp_millis()}
    /// 功能：通过实际SDK运行完整读取；输入场景号，输出快照或错误，结束后关闭本机测试服务器。
    async fn scenario(number:u8)->Result<Cache> {
        let listener=TcpListener::bind("127.0.0.1:0").await?;let address=listener.local_addr()?;
        let state=Arc::new(Fixture{scenario:number,personal_calls:AtomicUsize::new(0)});
        let app=Router::new().route("/mcp",post(respond)).with_state(state);
        let server=tokio::spawn(async move{let _=axum::serve(listener,app).await;});
        let work=async{
            let service=().serve(StreamableHttpClientTransport::from_uri(format!("http://{address}/mcp"))).await?;
            let tools=service.list_all_tools().await?;for name in TOOLS{let tool=tools.iter().find(|t|t.name==name).unwrap();validate_schema(name,&Value::Object((*tool.input_schema).clone()))?;}
            let client=AttendanceClient{service};let now=Utc.timestamp_millis_opt(millis("2026-11-09 20:00:00")).unwrap();
            client.read_period(&period(now),now,"fixture").await
        };
        let result=tokio::time::timeout(std::time::Duration::from_secs(8),work).await?;server.abort();result
    }
    macro_rules! rejected {($name:ident,$number:expr)=>{#[tokio::test] #[doc="Modify:通过实际MCP传输验证异常不能提交完整快照。"]async fn $name(){assert!(scenario($number).await.is_err());}};}
    rejected!(repeated_page_rejected,1);
    rejected!(empty_page_still_has_more_rejected,2);
    rejected!(raw_other_identity_rejected,3);
    rejected!(unknown_response_shape_rejected,4);
    rejected!(actual_tool_schema_change_rejected,5);
    rejected!(raw_actual_time_mismatch_rejected,6);
    rejected!(identity_changes_before_commit_rejected,7);
    rejected!(tool_error_not_empty_data,9);
    rejected!(missing_page_record_id_rejected,11);
    rejected!(raw_unfinished_page_rejected,12);
    #[tokio::test]
    /// 功能：验证完整分页与真实SDK的请求参数；无输入，输出通过或断言错误。
    async fn full_period_with_actual_sdk(){let cache=scenario(0).await.unwrap();assert!(cache.complete);let now=Utc.timestamp_millis_opt(millis("2026-11-09 20:00:00")).unwrap();let summary=summarize(&cache.period,&cache.days,now,true);assert_eq!(summary.total,1200);assert_eq!(summary.overtime,240);}
    #[tokio::test]
    /// 功能：验证批量期间收到下班卡时最终快照覆盖旧计时；无输入，输出断言。
    async fn final_personal_off_duty_replaces_running(){let cache=scenario(8).await.unwrap();let now=Utc.timestamp_millis_opt(millis("2026-11-09 20:00:00")).unwrap();let today=cache.days.iter().find(|d|d.date==today(now)).unwrap();assert_eq!(calculate_day(today,now).state,DayState::Complete);assert_eq!(calculate_day(today,now).minutes,calculate_day(today,now+Duration::minutes(2)).minutes);}
    #[tokio::test]
    /// 功能：验证同毫秒无效尝试不污染有效上班卡；无输入，输出断言。
    async fn personal_invalid_duplicate_keeps_clock_running(){let cache=scenario(10).await.unwrap();let now=Utc.timestamp_millis_opt(millis("2026-11-09 20:00:00")).unwrap();let today=cache.days.iter().find(|d|d.date==today(now)).unwrap();assert!(!today.review);assert_eq!(calculate_day(today,now).state,DayState::InProgress);}
}

#[cfg(test)]
mod oauth_tests {
    use super::*;
    use axum::{Router,Json,extract::State,routing::{get,post}};
    use base64::Engine as _;
    use sha2::{Digest,Sha256};
    use std::{collections::HashMap,sync::{Mutex,atomic::{AtomicUsize,Ordering}}};
    use rmcp::transport::auth::{AuthorizationMetadata,CredentialStore};
    struct Issuer {base:String,challenge:Mutex<String>,tokens:AtomicUsize,registrations:AtomicUsize}
    /// 功能：提供本机官方协议格式的发现信息；输入隔离发行方，输出OAuth元数据。
    async fn metadata(State(state):State<Arc<Issuer>>)->Json<Value>{Json(json!({"issuer":state.base,"authorization_endpoint":format!("{}/authorize",state.base),"token_endpoint":format!("{}/token",state.base),"registration_endpoint":format!("{}/register",state.base),"response_types_supported":["code"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["client_secret_post"],"authorization_response_iss_parameter_supported":true,"scopes_supported":["user"]}))}
    /// 功能：提供受保护资源发现；输入发行方，输出本地测试授权服务器位置。
    async fn resource(State(state):State<Arc<Issuer>>)->Json<Value>{Json(json!({"resource":state.base,"authorization_servers":[state.base]}))}
    /// 功能：验证程序实际DCR请求；输入隔离注册体，输出带secret的测试客户端，不创建任何外部凭证。
    async fn registration(State(state):State<Arc<Issuer>>,Json(body):Json<Value>)->Json<Value>{
        assert_eq!(body["application_type"],"native");assert_eq!(body["token_endpoint_auth_method"],"client_secret_post");
        assert_eq!(body["grant_types"],json!(["authorization_code","refresh_token"]));state.registrations.fetch_add(1,Ordering::SeqCst);
        Json(json!({"client_id":"unit-client","client_secret":"unit-secret","redirect_uris":body["redirect_uris"]}))
    }
    /// 功能：验证真实SDK的S256与secret_post交换/刷新；输入表单，输出仅测试用令牌。
    async fn token(State(state):State<Arc<Issuer>>,body:String)->Json<Value>{
        let url=reqwest::Url::parse(&format!("http://unit/?{body}")).unwrap();let form:HashMap<_,_>=url.query_pairs().into_owned().collect();
        assert_eq!(form.get("client_id").map(String::as_str),Some("unit-client"));assert_eq!(form.get("client_secret").map(String::as_str),Some("unit-secret"));
        if form["grant_type"]=="authorization_code" {
            let verifier=&form["code_verifier"];let challenge=base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            assert_eq!(challenge,*state.challenge.lock().unwrap());
        }else{assert_eq!(form["grant_type"],"refresh_token");assert_eq!(form["refresh_token"],"unit-refresh");}
        state.tokens.fetch_add(1,Ordering::SeqCst);
        Json(json!({"access_token":"unit-access","refresh_token":"unit-refresh","token_type":"Bearer","expires_in":3600,"scope":"user"}))
    }
    /// 功能：完整执行本机OAuth流；输入state/issuer错误场景，输出协议断言结果。
    async fn run(mode:u8){
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let base=format!("http://{}",listener.local_addr().unwrap());
        let issuer=Arc::new(Issuer{base:base.clone(),challenge:Mutex::new(String::new()),tokens:AtomicUsize::new(0),registrations:AtomicUsize::new(0)});
        let app=Router::new().route("/.well-known/oauth-protected-resource",get(resource)).route("/.well-known/oauth-authorization-server",get(metadata))
            .route("/register",post(registration)).route("/token",post(token)).with_state(issuer.clone());
        let server=tokio::spawn(async move{let _=axum::serve(listener,app).await;});
        let directory=tempfile::tempdir().unwrap();let store=Store::open(Some(directory.path().into())).unwrap();let vault=Vault(store.clone());
        let mut manager=AuthorizationManager::new_allowing_private_addresses(&base).await.unwrap();
        let discovery=manager.resolve_metadata().await.unwrap();assert!(discovery.source.is_discovered());
        let metadata:AuthorizationMetadata=discovery.metadata;manager.set_metadata(metadata.clone());manager.set_credential_store(vault.clone());
        let registration=register(&reqwest::Client::new(),&format!("{base}/register"),"http://127.0.0.1:40000/oauth/callback",base.clone()).await.unwrap();
        store.save_registration(registration.clone()).unwrap();
        manager.configure_client(OAuthClientConfig::new(registration.client_id.clone(),registration.redirect.clone()).with_client_secret(registration.client_secret.clone().unwrap())).unwrap();
        let url=manager.get_authorization_url(&["user"]).await.unwrap();let url=reqwest::Url::parse(&url).unwrap();let query:HashMap<_,_>=url.query_pairs().into_owned().collect();
        assert_eq!(query["code_challenge_method"],"S256");*issuer.challenge.lock().unwrap()=query["code_challenge"].clone();
        let csrf=if mode==1{"forged-state"}else{&query["state"]};let received=if mode==2{"http://wrong-issuer"}else{&base};
        let result=manager.exchange_code_for_token_with_issuer("unit-code",csrf,Some(received)).await;
        if mode!=0{assert!(result.is_err());assert_eq!(issuer.tokens.load(Ordering::SeqCst),0);server.abort();return;}
        result.unwrap();assert_eq!(issuer.registrations.load(Ordering::SeqCst),1);assert_eq!(issuer.tokens.load(Ordering::SeqCst),1);
        assert!(vault.load().await.unwrap().unwrap().token_response.is_some());assert!(store.has_tokens());
        let bytes=std::fs::read(store.root.join("oauth.credentials")).unwrap();assert!(!bytes.windows(b"unit-secret".len()).any(|p|p==b"unit-secret"));
        drop(manager);drop(vault);drop(store);
        let restarted=Store::open(Some(directory.path().into())).unwrap();let registration=restarted.registration().unwrap();
        let mut manager=AuthorizationManager::new_allowing_private_addresses(&base).await.unwrap();manager.set_metadata(metadata);manager.set_credential_store(Vault(restarted));
        assert!(manager.initialize_from_store().await.unwrap());manager.configure_client(OAuthClientConfig::new(registration.client_id,registration.redirect).with_client_secret(registration.client_secret.unwrap())).unwrap();
        manager.refresh_token().await.unwrap();assert_eq!(issuer.tokens.load(Ordering::SeqCst),2);server.abort();
    }
    #[tokio::test]
    /// 功能：验证本应用实际注册请求、SDK发现、PKCE、secret_post、DPAPI及冷启动刷新；无输入，输出断言。
    async fn discovery_registration_pkce_secret_post_cold_refresh(){run(0).await;}
    #[tokio::test]
    /// 功能：拒绝错误OAuth state且不交换令牌；无输入，输出断言。
    async fn rejects_wrong_state(){run(1).await;}
    #[tokio::test]
    /// 功能：拒绝错误OAuth issuer且不交换令牌；无输入，输出断言。
    async fn rejects_wrong_issuer(){run(2).await;}
}
