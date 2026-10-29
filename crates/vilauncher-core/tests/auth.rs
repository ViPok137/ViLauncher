mod common;

use common::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use vilauncher_core::auth::*;
use vilauncher_core::downloader::{build_client, CancelToken};
use vilauncher_core::secure_store::{MemoryBackend, TokenStore};
use vilauncher_core::CoreError;

const DEVICE: &str = r#"{"user_code":"ABCD1234","device_code":"DEV-CODE","verification_uri":"https://www.microsoft.com/link","expires_in":900,"interval":1,"message":"go"}"#;
const XBL: &str = r#"{"Token":"XBL-TOKEN","DisplayClaims":{"xui":[{"uhs":"UHS123"}]}}"#;
const XSTS: &str = r#"{"Token":"XSTS-TOKEN","DisplayClaims":{"xui":[{"uhs":"UHS123"}]}}"#;
const MCLOGIN: &str = r#"{"access_token":"MC-ACCESS","expires_in":86400}"#;
const PROFILE: &str = r#"{"id":"069a79f444e94726a5befca90e38aaf5","name":"Notch"}"#;

fn tok(access: &str, refresh: &str) -> String {
    format!(r#"{{"access_token":"{access}","refresh_token":"{refresh}","expires_in":3600}}"#)
}

fn chain(token_script: Vec<(u16, String)>) -> HashMap<String, Vec<(u16, String)>> {
    HashMap::from([
        ("/devicecode".to_string(), vec![(200, DEVICE.to_string())]),
        ("/token".to_string(), token_script),
        ("/xbl".to_string(), vec![(200, XBL.to_string())]),
        ("/xsts".to_string(), vec![(200, XSTS.to_string())]),
        ("/mclogin".to_string(), vec![(200, MCLOGIN.to_string())]),
        ("/mcprofile".to_string(), vec![(200, PROFILE.to_string())]),
    ])
}

fn manager(srv: &MockServer) -> (AuthManager, TokenStore, Arc<MemoryBackend>) {
    let mem = Arc::new(MemoryBackend::default());
    let store = TokenStore::new(mem.clone());
    let auth = MicrosoftAuth::new(build_client("test").unwrap(), "client-id")
        .with_endpoints(Endpoints::with_base(&srv.base()))
        .with_poll_interval(Duration::from_millis(5));
    (AuthManager::new(auth, store.clone()), store, mem)
}

#[tokio::test]
async fn device_login_polls_then_stores_refresh_token_in_keystore() {
    let srv = start_scripted(chain(vec![
        (400, r#"{"error":"authorization_pending"}"#.into()),
        (400, r#"{"error":"authorization_pending"}"#.into()),
        (200, tok("MS-ACCESS", "M.C5_REFRESH_1")),
    ]))
    .await;
    let (mgr, store, _mem) = manager(&srv);

    let shown = std::sync::Mutex::new(None::<String>);
    let session = mgr.login(|dc| *shown.lock().unwrap() = Some(dc.user_code.clone()), &CancelToken::new()).await.unwrap();

    assert_eq!(shown.lock().unwrap().as_deref(), Some("ABCD1234"));
    assert_eq!(session.username, "Notch");
    assert_eq!(session.uuid, "069a79f444e94726a5befca90e38aaf5");
    assert_eq!(session.access_token, "MC-ACCESS");
    assert!(!session.is_expired());
    assert_eq!(srv.hits("/token"), 3);
    assert_eq!(store.load_refresh_token().unwrap().as_deref(), Some("M.C5_REFRESH_1"));

    // access-токен не уходит во фронтенд
    let json = serde_json::to_string(&session).unwrap();
    assert!(!json.contains("MC-ACCESS") && json.contains("Notch"), "{json}");
}

#[tokio::test]
async fn restore_rotates_refresh_token() {
    let srv = start_scripted(chain(vec![(200, tok("MS-ACCESS-2", "M.C5_REFRESH_2"))])).await;
    let (mgr, store, _m) = manager(&srv);
    store.save_refresh_token("M.C5_REFRESH_1").unwrap();
    let s = mgr.restore().await.unwrap().expect("сессия");
    assert_eq!(s.username, "Notch");
    assert_eq!(store.load_refresh_token().unwrap().as_deref(), Some("M.C5_REFRESH_2"), "токен ротируется");
}

#[tokio::test]
async fn restore_without_token_is_none_and_makes_no_requests() {
    let srv = start_scripted(chain(vec![(200, tok("a", "b"))])).await;
    let (mgr, _s, _m) = manager(&srv);
    assert!(mgr.restore().await.unwrap().is_none());
    assert_eq!(srv.total_hits(), 0);
}

#[tokio::test]
async fn revoked_token_is_cleared() {
    let srv = start_scripted(chain(vec![(400, r#"{"error":"invalid_grant","error_description":"expired"}"#.into())])).await;
    let (mgr, store, mem) = manager(&srv);
    store.save_refresh_token("M.C5_DEAD").unwrap();
    assert!(mgr.restore().await.unwrap().is_none());
    assert!(mem.is_empty(), "мёртвый токен должен быть удалён из хранилища");
}

#[tokio::test]
async fn network_failure_keeps_token() {
    // Сервер не поднят на этом порту → ошибка соединения, токен обязан остаться.
    let mem = Arc::new(MemoryBackend::default());
    let store = TokenStore::new(mem.clone());
    let auth = MicrosoftAuth::new(build_client("test").unwrap(), "cid").with_endpoints(Endpoints::with_base("http://127.0.0.1:1"));
    let mgr = AuthManager::new(auth, store.clone());
    store.save_refresh_token("M.C5_KEEP").unwrap();
    assert!(mgr.restore().await.is_err());
    assert_eq!(store.load_refresh_token().unwrap().as_deref(), Some("M.C5_KEEP"));
}

#[tokio::test]
async fn declined_login_and_xbox_errors_are_reported_clearly() {
    let srv = start_scripted(chain(vec![(400, r#"{"error":"authorization_declined"}"#.into())])).await;
    let (mgr, store, _m) = manager(&srv);
    let err = mgr.login(|_| {}, &CancelToken::new()).await.unwrap_err();
    assert!(matches!(err, CoreError::Auth(ref m) if m.contains("отклонён")), "{err:?}");
    assert!(!store.has_refresh_token().unwrap());

    let mut script = chain(vec![(200, tok("a", "r"))]);
    script.insert("/xsts".into(), vec![(401, r#"{"XErr":2148916233}"#.into())]);
    let srv = start_scripted(script).await;
    let (mgr, store, _m) = manager(&srv);
    let err = mgr.login(|_| {}, &CancelToken::new()).await.unwrap_err();
    assert!(matches!(err, CoreError::Auth(ref m) if m.contains("профиля Xbox")), "{err:?}");
    assert!(!store.has_refresh_token().unwrap(), "при неудачном входе токен не сохраняется");
}

#[tokio::test]
async fn account_without_minecraft_license() {
    let mut script = chain(vec![(200, tok("a", "r"))]);
    script.insert("/mcprofile".into(), vec![(404, r#"{"error":"NOT_FOUND"}"#.into())]);
    let srv = start_scripted(script).await;
    let (mgr, _s, _m) = manager(&srv);
    let err = mgr.login(|_| {}, &CancelToken::new()).await.unwrap_err();
    assert!(matches!(err, CoreError::Auth(ref m) if m.contains("лицензии")), "{err:?}");
}

#[tokio::test]
async fn login_can_be_cancelled_and_logout_wipes_store() {
    let srv = start_scripted(chain(vec![(400, r#"{"error":"authorization_pending"}"#.into())])).await;
    let (mgr, store, mem) = manager(&srv);
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(40)).await; c2.cancel(); });
    assert!(matches!(mgr.login(|_| {}, &cancel).await.unwrap_err(), CoreError::Cancelled));

    store.save_refresh_token("M.C5_X").unwrap();
    mgr.logout().unwrap();
    assert!(mem.is_empty());
}
