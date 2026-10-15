//! Tests des Protokolls: Meldungen der Clients und Admin-Aktionen.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use vibedesk_console::{admin::insert_user, audit, db, now, router, AppState, Settings};

const PASSWORD: &str = "korrekt-pferd-batterie";
const UUID: &str = "dXVpZA==";

struct TestApp {
    app: Router,
    state: AppState,
}

impl TestApp {
    async fn new() -> Self {
        let pool = db::open_memory().await.unwrap();
        let state = AppState::new(pool, None, Settings::default());
        insert_user(&state.db, "anna", "", "admin", PASSWORD)
            .await
            .unwrap();
        Self {
            app: router(state.clone()),
            state,
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        cookie: Option<&str>,
    ) -> (StatusCode, String, Option<String>) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("x-vibedesk", "1");
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::to_owned);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned(), cookie)
    }

    async fn admin(&self) -> String {
        let (status, _, cookie) = self
            .call(
                Method::POST,
                "/admin/api/login",
                Some(json!({"username": "anna", "password": PASSWORD})),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        cookie.unwrap()
    }

    async fn logs(&self, kind: &str, cookie: &str) -> Vec<Value> {
        let (status, text, _) = self
            .call(
                Method::GET,
                &format!("/admin/api/logs/{kind}"),
                None,
                Some(cookie),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        serde_json::from_str(&text).unwrap()
    }

    async fn register_device(&self, id: &str) {
        let (_, text, _) = self
            .call(
                Method::POST,
                "/api/sysinfo",
                Some(json!({"id": id, "uuid": UUID, "hostname": "empfang-pc"})),
                None,
            )
            .await;
        assert_eq!(text, "SYSINFO_UPDATED");
    }
}

fn conn_event(extra: Value, nonce: &str) -> Value {
    let mut v = json!({"id": "111", "uuid": UUID, "conn_id": 7, "session_id": 42, "nonce": nonce});
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

#[tokio::test]
async fn connection_events_become_one_entry() {
    let t = TestApp::new().await;
    t.register_device("111").await;
    let admin = t.admin().await;

    for (event, nonce) in [
        (json!({"action": "new", "ip": "203.0.113.9"}), "n1"),
        (
            json!({"peer": ["222333444", "Laptop Bruno"], "type": 0, "primary_auth": 3, "two_factor": 1}),
            "n2",
        ),
        (json!({"action": "close"}), "n3"),
        // Wiederholte Meldung mit derselben nonce zählt nicht doppelt.
        (json!({"action": "new", "ip": "203.0.113.9"}), "n1"),
    ] {
        let (status, text, _) = t
            .call(
                Method::POST,
                "/api/audit/conn",
                Some(conn_event(event, nonce)),
                None,
            )
            .await;
        assert_eq!((status, text.as_str()), (StatusCode::OK, ""));
    }
    // Notiz des steuernden Geräts zur Sitzung.
    t.call(
        Method::POST,
        "/api/audit/conn",
        Some(json!({"id": "111", "session_id": 42, "note": "Drucker eingerichtet"})),
        None,
    )
    .await;

    let rows = t.logs("verbindungen", &admin).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let r = &rows[0];
    assert_eq!(r["device_id"], "111");
    assert_eq!(r["hostname"], "empfang-pc");
    assert_eq!(r["ip"], "203.0.113.9");
    assert_eq!(r["peer_id"], "222333444");
    assert_eq!(r["peer_name"], "Laptop Bruno");
    assert_eq!(r["conn_type"], 0);
    assert_eq!(r["primary_auth"], 3);
    assert_eq!(r["two_factor"], 1);
    assert_eq!(r["note"], "Drucker eingerichtet");
    assert!(r["ended_at"].is_number());
}

#[tokio::test]
async fn foreign_devices_cannot_write_the_log() {
    let t = TestApp::new().await;
    t.register_device("111").await;
    let admin = t.admin().await;
    let mut event = conn_event(json!({"action": "new", "ip": "198.51.100.1"}), "x1");
    event["uuid"] = json!("ZnJlbWQ=");
    let (status, _, _) = t
        .call(Method::POST, "/api/audit/conn", Some(event), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(t.logs("verbindungen", &admin).await.is_empty());
}

#[tokio::test]
async fn files_and_alarms_are_recorded() {
    let t = TestApp::new().await;
    t.register_device("111").await;
    let admin = t.admin().await;
    let info = json!({"ip": "203.0.113.9", "name": "Laptop Bruno", "num": 2, "files": [["=SUMME(A1)", 10], ["bericht.pdf", 5]]});
    let (status, _, _) = t
        .call(
            Method::POST,
            "/api/audit/file",
            Some(json!({"id": "111", "uuid": UUID, "peer_id": "222333444", "conn_id": 7, "type": 1,
                        "path": "C:\\Daten", "is_file": false, "info": info.to_string(), "nonce": "f1"})),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let alarm_info = json!({"id": "999", "name": "Unbekannt", "ip": "198.51.100.7"});
    t.call(
        Method::POST,
        "/api/audit/alarm",
        Some(json!({"id": "111", "uuid": UUID, "typ": 2, "info": alarm_info.to_string(), "conn_id": 8, "nonce": "a1"})),
        None,
    )
    .await;

    let files = t.logs("dateien", &admin).await;
    assert_eq!(files[0]["direction"], 1);
    assert_eq!(files[0]["num"], 2);
    assert_eq!(files[0]["peer_name"], "Laptop Bruno");
    let alarms = t.logs("alarme", &admin).await;
    assert_eq!(alarms[0]["typ"], 2);
    assert_eq!(alarms[0]["ip"], "198.51.100.7");

    // Im CSV werden Formeln entschärft.
    let (status, csv, _) = t
        .call(
            Method::GET,
            "/admin/api/logs/dateien/csv",
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(csv.starts_with("id,"), "{csv}");
    assert!(!csv.contains(",=SUMME"), "{csv}");
}

#[tokio::test]
async fn console_actions_are_logged_for_admins_only() {
    let t = TestApp::new().await;
    let admin = t.admin().await;
    t.call(
        Method::POST,
        "/admin/api/login",
        Some(json!({"username": "anna", "password": "falsch"})),
        None,
    )
    .await;
    t.call(
        Method::POST,
        "/admin/api/users",
        Some(json!({"username": "bruno", "role": "user", "password": PASSWORD})),
        Some(&admin),
    )
    .await;
    let actions: Vec<String> = t
        .logs("konsole", &admin)
        .await
        .iter()
        .map(|r| r["action"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        actions.contains(&"Anmeldung Konsole".to_owned()),
        "{actions:?}"
    );
    assert!(actions.contains(&"Anmeldung Konsole fehlgeschlagen".to_owned()));
    assert!(actions.contains(&"Benutzer angelegt".to_owned()));

    let (_, _, user_cookie) = t
        .call(
            Method::POST,
            "/admin/api/login",
            Some(json!({"username": "bruno", "password": PASSWORD})),
            None,
        )
        .await;
    let (status, _, _) = t
        .call(
            Method::GET,
            "/admin/api/logs/konsole",
            None,
            user_cookie.as_deref(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn old_entries_are_pruned() {
    let t = TestApp::new().await;
    sqlx::query("insert into admin_log (username, action, created_at) values ('alt', 'x', ?), ('neu', 'y', ?)")
        .bind(now() - 400 * 24 * 3600)
        .bind(now())
        .execute(&t.state.db)
        .await
        .unwrap();
    audit::prune(&t.state.db, 365).await.unwrap();
    let left: Vec<String> = sqlx::query_scalar("select username from admin_log")
        .fetch_all(&t.state.db)
        .await
        .unwrap();
    assert_eq!(left, vec!["neu".to_owned()]);
}
