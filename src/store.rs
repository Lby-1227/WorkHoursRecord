// Modify:独立Rust用户目录、DPAPI凭证及按身份周期隔离的原子存储。$saving-ai-agent-tokens
use crate::domain::{Cache, Claim, Period};
use anyhow::{Context, Result};
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{fs, path::{Path, PathBuf}, sync::{Arc, Mutex}};
use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN}};

pub const ENDPOINT: &str = "https://mcp-gw.dingtalk.com/oauth/server/attendance";
pub const MARKET: &str = "https://aihub.dingtalk.com/#/mcp-market/mcp";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings { pub opacity: f32, pub topmost: bool, pub floating_position: Option<(f32, f32)> }
impl Default for Settings {
    /// 功能：提供已确认的新安装默认外观；无输入，输出不透明且悬浮置顶的配置。
    fn default() -> Self { Self { opacity: 1., topmost: true, floating_position: None } }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection { pub session: String, pub user: String, pub connected_before: bool }
impl Default for Connection {
    /// 功能：建立新应用身份分区；无输入，输出随机分区且未连接的状态。
    fn default() -> Self { Self { session: uuid::Uuid::new_v4().simple().to_string(), user: String::new(), connected_before: false } }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Registration { pub client_id: String, pub client_secret: Option<String>, pub redirect: String, pub issuer: String }
#[derive(Clone, Default, Serialize, Deserialize)]
struct AuthFile { registration: Option<Registration>, credentials: Option<StoredCredentials> }
pub struct Store { pub root: PathBuf, writer: Mutex<()>, auth: Mutex<AuthFile> }
#[derive(Clone)]
pub struct Vault(pub Arc<Store>);

impl Store {
    /// 功能：打开本应用独立目录；输入可选隔离测试路径，输出校验过的存储，不读取WPF或Codex目录。
    pub fn open(root: Option<PathBuf>) -> Result<Arc<Self>> {
        let root = root.unwrap_or_else(|| PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("WorkHoursRecordRustGpui"));
        fs::create_dir_all(&root).context("无法创建Rust工时数据目录。")?;
        let encrypted = root.join("oauth.credentials");
        let auth = if encrypted.exists() { serde_json::from_slice(&protect(&fs::read(encrypted)?, false)?).context("本应用凭证无法读取，请保留文件。")? } else { AuthFile::default() };
        Ok(Arc::new(Self { root, writer: Mutex::new(()), auth: Mutex::new(auth) }))
    }
    /// 功能：读取JSON数据；输入文件名，输出可空对象，损坏内容明确失败。
    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.root.join(name);
        if !path.exists() { return Ok(None); }
        Ok(Some(serde_json::from_slice(&fs::read(path)?).with_context(|| format!("本应用{name}损坏，请保留文件后核对。"))?))
    }
    /// 功能：原子持久化JSON；输入本应用文件名及数据，输出写入结果，串行避免并发覆盖。
    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let _guard = self.writer.lock().map_err(|_| anyhow::anyhow!("数据写入锁不可用。"))?;
        atomic(&self.root.join(name), &serde_json::to_vec_pretty(value)?)
    }
    /// 功能：取得允许范围内的外观；无输入，输出默认或已保存设置。
    pub fn settings(&self) -> Result<Settings> {
        let mut settings = self.read::<Settings>("settings.json")?.unwrap_or_default();
        settings.opacity = if settings.opacity.is_finite() { settings.opacity.clamp(0.2, 1.) } else { 1. }; Ok(settings)
    }
    /// 功能：取得连接状态；无输入，输出已有或新建身份分区。
    pub fn connection(&self) -> Result<Connection> { Ok(self.read("connection.json")?.unwrap_or_default()) }
    /// 功能：生成身份与周期的文件名；输入身份及周期，输出哈希文件名。
    fn cache_name(identity: &str, period: &Period) -> String {
        let hash: String = Sha256::digest(identity.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
        format!("attendance-{}-{}.json", &hash[..24], period.start.format("%Y%m%d"))
    }
    /// 功能：读取指定身份当期缓存；输入身份及周期，输出验证过的快照或空值。
    pub fn cache(&self, identity: &str, period: &Period) -> Result<Option<Cache>> {
        let cache: Option<Cache> = self.read(&Self::cache_name(identity, period))?;
        anyhow::ensure!(cache.as_ref().is_none_or(|c| c.identity == identity && c.period == *period), "缓存身份或周期不匹配。"); Ok(cache)
    }
    /// 功能：替换完整快照；输入缓存，输出落盘结果，不追加重复日期。
    pub fn save_cache(&self, cache: &Cache) -> Result<()> { self.write(&Self::cache_name(&cache.identity, &cache.period), cache) }
    /// 功能：读取领取历史；无输入，输出记录，损坏时不会伪装为空。
    pub fn claims(&self) -> Result<Vec<Claim>> { Ok(self.read("claims.json")?.unwrap_or_default()) }
    /// 功能：读取本应用注册信息；无输入，输出内存中的凭证配置，不记录日志。
    pub fn registration(&self) -> Option<Registration> { self.auth.lock().ok().and_then(|a| a.registration.clone()) }
    /// 功能：保存本应用动态注册信息并清除旧令牌；输入注册对象，输出用户级加密保存结果。
    pub fn save_registration(&self, registration: Registration) -> Result<()> {
        let mut auth = self.auth.lock().map_err(|_| anyhow::anyhow!("凭证锁不可用。"))?;
        let next = AuthFile { registration: Some(registration), credentials: None };
        self.write_auth(&next)?; *auth = next; Ok(())
    }
    /// 功能：判断是否有本应用令牌；无输入，输出是否可以后台恢复。
    pub fn has_tokens(&self) -> bool { self.auth.lock().is_ok_and(|auth| auth.credentials.as_ref().is_some_and(|c| c.token_response.is_some())) }
    /// 功能：加密写入本应用凭证；输入凭证状态，输出原子保存结果，调用者已持有内存锁。
    fn write_auth(&self, auth: &AuthFile) -> Result<()> {
        let _guard = self.writer.lock().map_err(|_| anyhow::anyhow!("凭证写入锁不可用。"))?;
        atomic(&self.root.join("oauth.credentials"), &protect(&serde_json::to_vec(auth)?, true)?)
    }
}

#[async_trait::async_trait]
impl CredentialStore for Vault {
    /// 功能：供SDK读取内存令牌；无业务输入，输出本应用令牌副本，无重复磁盘IO。
    async fn load(&self) -> std::result::Result<Option<StoredCredentials>, AuthError> {
        self.0.auth.lock().map(|auth| auth.credentials.clone()).map_err(|_| AuthError::CredentialStoreError("凭证锁不可用".into()))
    }
    /// 功能：供SDK保存授权或刷新后的令牌；输入凭证，输出加密持久化结果。
    async fn save(&self, credentials: StoredCredentials) -> std::result::Result<(), AuthError> {
        let mut auth = self.0.auth.lock().map_err(|_| AuthError::CredentialStoreError("凭证锁不可用".into()))?;
        let mut next = auth.clone(); next.credentials = Some(credentials);
        self.0.write_auth(&next).map_err(|_| AuthError::CredentialStoreError("凭证保存失败".into()))?; *auth = next; Ok(())
    }
    /// 功能：供SDK清除失效令牌；无输入，输出保存结果，不删除领取或历史缓存。
    async fn clear(&self) -> std::result::Result<(), AuthError> {
        let mut auth = self.0.auth.lock().map_err(|_| AuthError::CredentialStoreError("凭证锁不可用".into()))?;
        let mut next = auth.clone(); next.credentials = None;
        self.0.write_auth(&next).map_err(|_| AuthError::CredentialStoreError("凭证保存失败".into()))?; *auth = next; Ok(())
    }
}
/// 功能：同目录原子替换文件；输入完整路径及字节，输出保存结果，刷新到磁盘后再替换。
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_extension("tmp"); let mut file = fs::File::create(&temporary)?;
    file.write_all(bytes)?; file.sync_all()?; drop(file); fs::rename(temporary, path)?; Ok(())
}
/// 功能：调用当前Windows用户DPAPI；输入字节和加密标识，输出加密/解密字节，绝不降级明文保存。
pub fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: bytes.len().try_into()?, pbData: bytes.as_ptr() as *mut u8 };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
    let success = unsafe { if encrypt { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output) }
        else { CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output) } };
    anyhow::ensure!(success != 0, "当前Windows用户无法处理本应用加密凭证。");
    let result = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut _); } Ok(result)
}
