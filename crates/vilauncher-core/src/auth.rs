//! Авторизация Microsoft → Xbox Live → XSTS → Minecraft Services (device code flow).
//!
//! Device code не требует redirect URI и локального HTTP-сервера: лаунчер показывает код,
//! игрок вводит его на https://microsoft.com/link. Refresh-токен сразу уходит в системное
//! хранилище (`secure_store`), access-токен Minecraft живёт только в памяти.
//!
//! ВАЖНО: нужен Azure App Registration (public client, "Allow public client flows") и,
//! для новых приложений, одобрение Mojang на доступ к Minecraft API — см. README_RUST.md.

use crate::downloader::CancelToken;
use crate::error::{CoreError, Result};
use crate::secure_store::TokenStore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCOPE: &str = "XboxLive.signin offline_access";

#[derive(Clone, Debug)]
pub struct Endpoints {
    pub device_code: String,
    pub token: String,
    pub xbl: String,
    pub xsts: String,
    pub mc_login: String,
    pub mc_profile: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            device_code: "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode".into(),
            token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".into(),
            xbl: "https://user.auth.xboxlive.com/user/authenticate".into(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".into(),
            mc_login: "https://api.minecraftservices.com/authentication/login_with_xbox".into(),
            mc_profile: "https://api.minecraftservices.com/minecraft/profile".into(),
        }
    }
}

impl Endpoints {
    /// Все endpoint'ы на один базовый URL (для тестов с mock-сервером).
    pub fn with_base(base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self {
            device_code: format!("{b}/devicecode"),
            token: format!("{b}/token"),
            xbl: format!("{b}/xbl"),
            xsts: format!("{b}/xsts"),
            mc_login: format!("{b}/mclogin"),
            mc_profile: format!("{b}/mcprofile"),
        }
    }
}

/// Данные для показа игроку.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub message: Option<String>,
}
fn default_interval() -> u64 {
    5
}

/// Сессия Minecraft. `access_token` НЕ сериализуется во фронтенд.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McSession {
    pub username: String,
    pub uuid: String,
    #[serde(skip_serializing)]
    pub access_token: String,
    /// Unix-время (сек), когда истечёт access-токен.
    pub expires_at: u64,
}

impl McSession {
    pub fn is_expired(&self) -> bool {
        now_secs() + 60 >= self.expires_at
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: Option<String>,
    refresh_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct XblResp {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    claims: XblClaims,
}
#[derive(Deserialize)]
struct XblClaims {
    xui: Vec<XblUhs>,
}
#[derive(Deserialize)]
struct XblUhs {
    uhs: String,
}
#[derive(Deserialize)]
struct McLoginResp {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}
#[derive(Deserialize)]
struct McProfile {
    id: String,
    name: String,
}

fn xerr_message(code: u64) -> &'static str {
    match code {
        2148916233 => "У этой учётной записи Microsoft нет профиля Xbox. Создай его на xbox.com и повтори вход.",
        2148916235 => "Xbox Live недоступен в стране этой учётной записи.",
        2148916236 | 2148916237 => "Для учётной записи требуется подтверждение возраста (Xbox).",
        2148916238 => "Детская учётная запись: добавь её в семейную группу Microsoft и разреши онлайн-игру.",
        _ => "Xbox Live отклонил запрос.",
    }
}

pub struct MicrosoftAuth {
    client: reqwest::Client,
    client_id: String,
    endpoints: Endpoints,
    poll_override: Option<Duration>,
}

impl MicrosoftAuth {
    pub fn new(client: reqwest::Client, client_id: impl Into<String>) -> Self {
        Self { client, client_id: client_id.into(), endpoints: Endpoints::default(), poll_override: None }
    }
    pub fn with_endpoints(mut self, e: Endpoints) -> Self {
        self.endpoints = e;
        self
    }
    /// Только для тестов: фиксированный интервал опроса вместо `interval` сервера.
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_override = Some(d);
        self
    }

    pub async fn begin_device_login(&self) -> Result<DeviceCode> {
        if self.client_id.trim().is_empty() {
            return Err(CoreError::Auth("Не задан Azure client_id (MS_CLIENT_ID)".into()));
        }
        let resp = self
            .client
            .post(&self.endpoints.device_code)
            .form(&[("client_id", self.client_id.as_str()), ("scope", SCOPE)])
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(CoreError::Auth(format!("devicecode: HTTP {status}: {}", short(&text))));
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Ждёт, пока игрок подтвердит код. Возвращает сессию и НОВЫЙ refresh-токен.
    pub async fn complete_device_login(&self, dc: &DeviceCode, cancel: &CancelToken) -> Result<(McSession, String)> {
        let started = std::time::Instant::now();
        let mut interval = self.poll_override.unwrap_or(Duration::from_secs(dc.interval.max(1)));
        loop {
            if cancel.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            if started.elapsed() > Duration::from_secs(dc.expires_in) {
                return Err(CoreError::Auth("Код входа истёк. Начни вход заново.".into()));
            }
            tokio::time::sleep(interval).await;

            let t = self
                .token_request(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", self.client_id.as_str()),
                    ("device_code", dc.device_code.as_str()),
                ])
                .await?;
            match t.error.as_deref() {
                None => {
                    let refresh = t.refresh_token.ok_or_else(|| CoreError::Auth("нет refresh_token в ответе".into()))?;
                    let ms = t.access_token.ok_or_else(|| CoreError::Auth("нет access_token в ответе".into()))?;
                    return Ok((self.minecraft_session(&ms).await?, refresh));
                }
                Some("authorization_pending") => {}
                Some("slow_down") => interval += Duration::from_secs(5),
                Some("authorization_declined") | Some("access_denied") => {
                    return Err(CoreError::Auth("Вход отклонён в окне Microsoft.".into()))
                }
                Some("expired_token") | Some("bad_verification_code") => {
                    return Err(CoreError::Auth("Код входа истёк. Начни вход заново.".into()))
                }
                Some(other) => {
                    return Err(CoreError::Auth(format!("{other}: {}", t.error_description.unwrap_or_default())))
                }
            }
        }
    }

    /// Обновление по refresh-токену. Microsoft ротирует токен — сохрани возвращённый.
    /// `invalid_grant` / `interaction_required` → `AuthRequired` (нужен новый интерактивный вход).
    pub async fn refresh(&self, refresh_token: &str) -> Result<(McSession, String)> {
        let t = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("refresh_token", refresh_token),
                ("scope", SCOPE),
            ])
            .await?;
        if let Some(err) = t.error.as_deref() {
            return match err {
                "invalid_grant" | "interaction_required" | "invalid_request" | "unauthorized_client" => {
                    Err(CoreError::AuthRequired)
                }
                other => Err(CoreError::Auth(format!("{other}: {}", t.error_description.unwrap_or_default()))),
            };
        }
        let ms = t.access_token.ok_or_else(|| CoreError::Auth("нет access_token в ответе".into()))?;
        let new_refresh = t.refresh_token.unwrap_or_else(|| refresh_token.to_string());
        Ok((self.minecraft_session(&ms).await?, new_refresh))
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> Result<TokenResp> {
        // Ответ разбираем независимо от HTTP-статуса: ошибки OAuth приходят как 400 + JSON.
        let text = self.client.post(&self.endpoints.token).form(form).send().await?.text().await?;
        serde_json::from_str(&text).map_err(|_| CoreError::Auth(format!("token endpoint: {}", short(&text))))
    }

    /// MS access token → XBL → XSTS → Minecraft access token → профиль.
    pub async fn minecraft_session(&self, ms_access_token: &str) -> Result<McSession> {
        // 1. Xbox Live
        let xbl: XblResp = self
            .post_json(
                &self.endpoints.xbl,
                &json!({
                    "Properties": { "AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com",
                                    "RpsTicket": format!("d={ms_access_token}") },
                    "RelyingParty": "http://auth.xboxlive.com",
                    "TokenType": "JWT"
                }),
                "Xbox Live",
            )
            .await?;
        // 2. XSTS
        let xsts: XblResp = self
            .post_json(
                &self.endpoints.xsts,
                &json!({
                    "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl.token] },
                    "RelyingParty": "rp://api.minecraftservices.com/",
                    "TokenType": "JWT"
                }),
                "XSTS",
            )
            .await?;
        let uhs = xsts.claims.xui.first().map(|x| x.uhs.clone()).ok_or_else(|| CoreError::Auth("XSTS: нет uhs".into()))?;
        // 3. Minecraft
        let mc: McLoginResp = self
            .post_json(
                &self.endpoints.mc_login,
                &json!({ "identityToken": format!("XBL3.0 x={uhs};{}", xsts.token) }),
                "Minecraft login",
            )
            .await?;
        // 4. Профиль (404 = на аккаунте нет лицензии Java Edition)
        let resp = self.client.get(&self.endpoints.mc_profile).bearer_auth(&mc.access_token).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(CoreError::Auth("На этой учётной записи нет лицензии Minecraft: Java Edition.".into()));
        }
        let profile: McProfile = resp.error_for_status()?.json().await?;
        Ok(McSession {
            username: profile.name,
            uuid: profile.id,
            access_token: mc.access_token,
            expires_at: now_secs() + if mc.expires_in == 0 { 86_400 } else { mc.expires_in },
        })
    }

    async fn post_json<T: for<'de> Deserialize<'de>>(&self, url: &str, body: &serde_json::Value, what: &str) -> Result<T> {
        let resp = self.client.post(url).header("Accept", "application/json").json(body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(x) = v.get("XErr").and_then(|x| x.as_u64()) {
                    return Err(CoreError::Auth(format!("{what}: {}", xerr_message(x))));
                }
            }
            return Err(CoreError::Auth(format!("{what}: HTTP {status}: {}", short(&text))));
        }
        serde_json::from_str(&text).map_err(|e| CoreError::Auth(format!("{what}: непонятный ответ ({e})")))
    }
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Высокоуровневый менеджер: логин, восстановление сессии при старте, выход.
pub struct AuthManager {
    pub auth: MicrosoftAuth,
    pub store: TokenStore,
}

impl AuthManager {
    pub fn new(auth: MicrosoftAuth, store: TokenStore) -> Self {
        Self { auth, store }
    }

    /// Интерактивный вход. `on_code` вызывается один раз, когда нужно показать код игроку.
    pub async fn login(&self, on_code: impl FnOnce(DeviceCode), cancel: &CancelToken) -> Result<McSession> {
        let dc = self.auth.begin_device_login().await?;
        on_code(dc.clone());
        let (session, refresh) = self.auth.complete_device_login(&dc, cancel).await?;
        self.store.save_refresh_token(&refresh)?;
        Ok(session)
    }

    /// Тихое восстановление при запуске. `Ok(None)` — токена нет или он больше не действует.
    /// Сетевые сбои возвращаются как `Err` и токен НЕ удаляют (игрок просто офлайн).
    pub async fn restore(&self) -> Result<Option<McSession>> {
        let Some(token) = self.store.load_refresh_token()? else { return Ok(None) };
        match self.auth.refresh(&token).await {
            Ok((session, new_refresh)) => {
                if new_refresh != token {
                    self.store.save_refresh_token(&new_refresh)?;
                }
                Ok(Some(session))
            }
            Err(CoreError::AuthRequired) => {
                self.store.clear()?;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub fn logout(&self) -> Result<()> {
        self.store.clear()
    }
}
