//! Tests der Schnittstelle, die der VibeDesk-Client anspricht: Anmeldung,
//! Adressbücher und Geräteliste.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use vibedesk_console::{admin::insert_user, db, now, router, totp, AppState, Settings};

const PASSWORD: &str = "korrekt-pferd-batterie";

struct TestApp {
    app: Router,
    state: AppState,
}

struct Reply {
    status: StatusCode,
    body: Value,
    text: String,
    cookie: Option<String>,
}

enum Auth<'a> {
    None,
    Bearer(&'a str),
    Cookie(&'a str),
}

impl TestApp {
    async fn new() -> Self {
        let pool = db::open_memory().await.unwrap();
        let state = AppState::new(pool, None, Settings::default());
        Self {
            app: router(state.clone()),
            state,
        }
    }

    async fn user(&self, name: &str, role: &str) -> i64 {
        insert_user(&self.state.db, name, "", role, PASSWORD)
            .await
            .unwrap()
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>, auth: Auth<'_>) -> Reply {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("x-vibedesk", "1");
        match auth {
            Auth::None => {}
            Auth::Bearer(t) => req = req.header(header::AUTHORIZATION, format!("Bearer {t}")),
            Auth::Cookie(c) => req = req.header(header::COOKIE, c),
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
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Reply {
            status,
            body,
            text,
            cookie,
        }
    }

    async fn client_login(&self, name: &str, device: &str) -> String {
        let r = self
            .call(
                Method::POST,
                "/api/login",
                Some(
                    json!({"username": name, "password": PASSWORD, "id": device, "uuid": "dXVpZA==",
                            "autoLogin": true, "type": "account"}),
                ),
                Auth::None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        assert_eq!(r.body["type"], "access_token");
        r.body["access_token"].as_str().unwrap().to_owned()
    }

    async fn admin_cookie(&self, name: &str) -> String {
        let r = self
            .call(
                Method::POST,
                "/admin/api/login",
                Some(json!({"username": name, "password": PASSWORD})),
                Auth::None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        r.cookie.unwrap()
    }
}

#[tokio::test]
async fn login_current_user_and_logout() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;

    let r = t
        .call(Method::GET, "/api/login-options", None, Auth::None)
        .await;
    assert_eq!(r.body, json!([]));

    let r = t
        .call(
            Method::POST,
            "/api/login",
            Some(json!({"username": "anna", "password": "falsch", "type": "account"})),
            Auth::None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.body["error"].is_string());

    let token = t.client_login("anna", "111").await;
    let r = t
        .call(
            Method::POST,
            "/api/currentUser",
            Some(json!({"id": "111"})),
            Auth::Bearer(&token),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["name"], "anna");
    assert_eq!(r.body["is_admin"], true);
    assert_eq!(r.body["status"], 1);

    let r = t
        .call(
            Method::POST,
            "/api/logout",
            Some(json!({"id": "111"})),
            Auth::Bearer(&token),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = t
        .call(
            Method::POST,
            "/api/currentUser",
            Some(json!({})),
            Auth::Bearer(&token),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn two_factor_client_login() {
    let t = TestApp::new().await;
    t.user("anna", "user").await;
    let secret = totp::generate_secret();
    sqlx::query("update users set totp_secret = ?")
        .bind(&secret)
        .execute(&t.state.db)
        .await
        .unwrap();

    let r = t
        .call(
            Method::POST,
            "/api/login",
            Some(json!({"username": "anna", "password": PASSWORD, "type": "account"})),
            Auth::None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["type"], "email_check");
    assert_eq!(r.body["tfa_type"], "tfa_check");
    assert!(r.body["access_token"].is_null());
    let challenge = r.body["secret"].as_str().unwrap().to_owned();

    let step = |code: &str| {
        json!({"verificationCode": code, "tfaCode": code, "secret": challenge,
               "username": "anna", "type": "email_code"})
    };
    let r = t
        .call(Method::POST, "/api/login", Some(step("000000")), Auth::None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let code = totp::code(&secret, now()).unwrap();
    let r = t
        .call(Method::POST, "/api/login", Some(step(&code)), Auth::None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    assert_eq!(r.body["type"], "access_token");

    // Die Herausforderung gilt nur einmal.
    let r = t
        .call(Method::POST, "/api/login", Some(step(&code)), Auth::None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn disabling_a_user_ends_client_logins() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    let bruno = t.user("bruno", "user").await;
    let token = t.client_login("bruno", "222").await;
    let admin = t.admin_cookie("anna").await;

    let r = t
        .call(
            Method::PATCH,
            &format!("/admin/api/users/{bruno}"),
            Some(json!({"disabled": true})),
            Auth::Cookie(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = t
        .call(
            Method::POST,
            "/api/currentUser",
            Some(json!({})),
            Auth::Bearer(&token),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn personal_address_book() {
    let t = TestApp::new().await;
    t.user("anna", "user").await;
    let token = t.client_login("anna", "111").await;
    let auth = || Auth::Bearer(&token);

    let r = t.call(Method::POST, "/api/ab/settings", None, auth()).await;
    assert_eq!(r.body["max_peer_one_ab"], 0);
    let r = t.call(Method::POST, "/api/ab/personal", None, auth()).await;
    let guid = r.body["guid"].as_str().unwrap().to_owned();
    let again = t.call(Method::POST, "/api/ab/personal", None, auth()).await;
    assert_eq!(again.body["guid"], guid.as_str());

    // Ändernde Endpunkte antworten mit 200 und leerem Body.
    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/tag/add/{guid}"),
            Some(json!({"name": "Büro", "color": 4294198070u32})),
            auth(),
        )
        .await;
    assert_eq!((r.status, r.text.as_str()), (StatusCode::OK, ""));
    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/peer/add/{guid}"),
            Some(
                json!({"id": "123456789", "alias": "Empfang", "tags": ["Büro", "Unbekannt"],
                        "hash": "abc", "password": "geheim", "platform": "Windows", "evil": "x"}),
            ),
            auth(),
        )
        .await;
    assert_eq!((r.status, r.text.as_str()), (StatusCode::OK, ""));

    let r = t
        .call(
            Method::PUT,
            &format!("/api/ab/peer/update/{guid}"),
            Some(json!({"id": "123456789", "note": "Drucker daneben"})),
            auth(),
        )
        .await;
    assert_eq!((r.status, r.text.as_str()), (StatusCode::OK, ""));

    let r = t
        .call(
            Method::PUT,
            &format!("/api/ab/tag/rename/{guid}"),
            Some(json!({"old": "Büro", "new": "Praxis"})),
            auth(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);

    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/peers?current=1&pageSize=100&ab={guid}"),
            None,
            auth(),
        )
        .await;
    assert_eq!(r.body["total"], 1);
    let peer = &r.body["data"][0];
    assert_eq!(peer["id"], "123456789");
    assert_eq!(peer["alias"], "Empfang");
    assert_eq!(peer["note"], "Drucker daneben");
    assert_eq!(peer["hash"], "abc");
    assert_eq!(peer["tags"], json!(["Praxis"]));
    assert!(peer.get("password").is_none());
    assert!(peer.get("evil").is_none());

    let r = t
        .call(Method::POST, &format!("/api/ab/tags/{guid}"), None, auth())
        .await;
    assert_eq!(r.body, json!([{"name": "Praxis", "color": 4294198070u32}]));

    let r = t
        .call(
            Method::DELETE,
            &format!("/api/ab/tag/{guid}"),
            Some(json!(["Praxis"])),
            auth(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = t
        .call(
            Method::DELETE,
            &format!("/api/ab/peer/{guid}"),
            Some(json!(["123456789"])),
            auth(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/peers?ab={guid}"),
            None,
            auth(),
        )
        .await;
    assert_eq!(r.body["total"], 0);
}

#[tokio::test]
async fn shared_address_books_respect_rules() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    let bruno = t.user("bruno", "user").await;
    let carla = t.user("carla", "user").await;
    let admin = t.admin_cookie("anna").await;

    let r = t
        .call(
            Method::POST,
            "/admin/api/address-books",
            Some(json!({"name": "Kunden", "note": "Alle Kunden"})),
            Auth::Cookie(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let guid = r.body["guid"].as_str().unwrap().to_owned();
    for (user, rule) in [(bruno, 2), (carla, 1)] {
        let r = t
            .call(
                Method::PUT,
                &format!("/admin/api/address-books/{guid}/members/{user}"),
                Some(json!({"rule": rule})),
                Auth::Cookie(&admin),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    }

    let bruno_token = t.client_login("bruno", "222").await;
    let carla_token = t.client_login("carla", "333").await;
    let r = t
        .call(
            Method::POST,
            "/api/ab/shared/profiles?current=1&pageSize=100",
            None,
            Auth::Bearer(&carla_token),
        )
        .await;
    assert_eq!(r.body["total"], 1);
    assert_eq!(r.body["data"][0]["name"], "Kunden");
    assert_eq!(r.body["data"][0]["owner"], "anna");
    assert_eq!(r.body["data"][0]["rule"], 1);

    let add = json!({"id": "555", "password": "gemeinsam", "hash": "nicht-speichern"});
    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/peer/add/{guid}"),
            Some(add.clone()),
            Auth::Bearer(&carla_token),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/peer/add/{guid}"),
            Some(add),
            Auth::Bearer(&bruno_token),
        )
        .await;
    assert_eq!((r.status, r.text.as_str()), (StatusCode::OK, ""));

    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/peers?ab={guid}"),
            None,
            Auth::Bearer(&carla_token),
        )
        .await;
    assert_eq!(r.body["data"][0]["password"], "gemeinsam");
    assert!(r.body["data"][0].get("hash").is_none());

    // Fremde persönliche Adressbücher sind tabu.
    let personal = t
        .call(
            Method::POST,
            "/api/ab/personal",
            None,
            Auth::Bearer(&bruno_token),
        )
        .await;
    let other = personal.body["guid"].as_str().unwrap();
    let r = t
        .call(
            Method::POST,
            &format!("/api/ab/peers?ab={other}"),
            None,
            Auth::Bearer(&carla_token),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Die Konsole zeigt Einträge ohne Passwörter.
    let r = t
        .call(
            Method::GET,
            &format!("/admin/api/address-books/{guid}/peers"),
            None,
            Auth::Cookie(&admin),
        )
        .await;
    assert_eq!(r.body[0]["id"], "555");
    assert!(r.body[0].get("password").is_none());
}

#[tokio::test]
async fn device_list_follows_logins() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    t.user("bruno", "user").await;
    // Gerät meldet sich per Heartbeat, danach meldet sich bruno darauf an.
    let r = t
        .call(
            Method::POST,
            "/api/sysinfo",
            Some(json!({"id": "222", "uuid": "dXVpZA==", "hostname": "bruno-pc", "os": "Windows 11 / 24H2"})),
            Auth::None,
        )
        .await;
    assert_eq!(r.text, "SYSINFO_UPDATED");
    let bruno = t.client_login("bruno", "222").await;
    let anna = t.client_login("anna", "111").await;

    let r = t
        .call(
            Method::GET,
            "/api/peers?current=1&pageSize=100&accessible=&status=1",
            None,
            Auth::Bearer(&bruno),
        )
        .await;
    assert_eq!(r.body["total"], 1);
    assert_eq!(r.body["data"][0]["id"], "222");
    assert_eq!(r.body["data"][0]["user_name"], "bruno");
    assert_eq!(r.body["data"][0]["info"]["device_name"], "bruno-pc");

    let r = t
        .call(
            Method::GET,
            "/api/users?current=1&pageSize=100&accessible=&status=1",
            None,
            Auth::Bearer(&bruno),
        )
        .await;
    assert_eq!(r.body["total"], 1);
    let r = t
        .call(
            Method::GET,
            "/api/users?current=1&pageSize=100",
            None,
            Auth::Bearer(&anna),
        )
        .await;
    assert_eq!(r.body["total"], 2);

    let r = t
        .call(
            Method::GET,
            "/api/device-group/accessible?current=1&pageSize=100",
            None,
            Auth::Bearer(&anna),
        )
        .await;
    assert_eq!(r.body, json!({"total": 0, "data": []}));
}
