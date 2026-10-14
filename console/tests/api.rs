use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use vibedesk_console::{
    admin::insert_user, db, hbbs::HbbsDb, now, router, totp, AppState, Settings,
};

const PASSWORD: &str = "korrekt-pferd-batterie";

struct TestApp {
    app: Router,
    state: AppState,
}

struct Reply {
    status: StatusCode,
    cookie: Option<String>,
    body: Value,
    text: String,
}

impl TestApp {
    async fn new() -> Self {
        Self::with_hbbs(None).await
    }

    async fn with_hbbs(hbbs: Option<HbbsDb>) -> Self {
        let pool = db::open_memory().await.unwrap();
        let state = AppState::new(pool, hbbs, Settings::default());
        Self {
            app: router(state.clone()),
            state,
        }
    }

    async fn user(&self, name: &str, role: &str) {
        insert_user(&self.state.db, name, "", role, PASSWORD)
            .await
            .unwrap();
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        cookie: Option<&str>,
    ) -> Reply {
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
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Reply {
            status,
            cookie,
            body,
            text,
        }
    }

    async fn login(&self, name: &str) -> String {
        let r = self
            .call(
                Method::POST,
                "/admin/api/login",
                Some(json!({"username": name, "password": PASSWORD})),
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text);
        r.cookie.expect("Cookie")
    }
}

#[tokio::test]
async fn changes_need_the_csrf_header() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"username": "anna", "password": PASSWORD}).to_string(),
        ))
        .unwrap();
    let res = t.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn login_session_and_logout() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;

    let r = t
        .call(
            Method::POST,
            "/admin/api/login",
            Some(json!({"username": "anna", "password": "falsch"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        t.call(Method::GET, "/admin/api/me", None, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    let cookie = t.login("ANNA").await;
    let r = t
        .call(Method::GET, "/admin/api/me", None, Some(&cookie))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["username"], "anna");
    assert_eq!(r.body["role"], "admin");

    t.call(Method::POST, "/admin/api/logout", None, Some(&cookie))
        .await;
    assert_eq!(
        t.call(Method::GET, "/admin/api/me", None, Some(&cookie))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn too_many_failures_lock_the_account() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    for _ in 0..5 {
        t.call(
            Method::POST,
            "/admin/api/login",
            Some(json!({"username": "anna", "password": "falsch"})),
            None,
        )
        .await;
    }
    let r = t
        .call(
            Method::POST,
            "/admin/api/login",
            Some(json!({"username": "anna", "password": PASSWORD})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn two_factor_login() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    let cookie = t.login("anna").await;

    let r = t
        .call(
            Method::POST,
            "/admin/api/me/2fa/setup",
            Some(json!({"password": PASSWORD})),
            Some(&cookie),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);
    let secret = r.body["secret"].as_str().unwrap().to_owned();
    assert!(r.body["qr_svg"].as_str().unwrap().contains("<svg"));

    let r = t
        .call(
            Method::POST,
            "/admin/api/me/2fa/enable",
            Some(json!({"code": "000000"})),
            Some(&cookie),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Code des vorherigen Zeitschritts, damit er beim Login nicht als bereits verwendet gilt.
    let earlier = totp::code(&secret, now() - 30).unwrap();
    let r = t
        .call(
            Method::POST,
            "/admin/api/me/2fa/enable",
            Some(json!({"code": earlier})),
            Some(&cookie),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);

    let login = |code: &str| json!({"username": "anna", "password": PASSWORD, "code": code});
    let r = t
        .call(
            Method::POST,
            "/admin/api/login",
            Some(json!({"username": "anna", "password": PASSWORD})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["needs_code"], true);

    let code = totp::code(&secret, now()).unwrap();
    let r = t
        .call(Method::POST, "/admin/api/login", Some(login(&code)), None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text);

    // Derselbe Code ein zweites Mal wird abgelehnt.
    let r = t
        .call(Method::POST, "/admin/api/login", Some(login(&code)), None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn users_cannot_reach_admin_endpoints() {
    let t = TestApp::new().await;
    t.user("bruno", "user").await;
    let cookie = t.login("bruno").await;
    assert_eq!(
        t.call(Method::GET, "/admin/api/users", None, Some(&cookie))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.call(Method::GET, "/admin/api/devices", None, Some(&cookie))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.call(Method::GET, "/admin/api/me", None, Some(&cookie))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn admin_manages_users() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    let admin = t.login("anna").await;

    let r = t
        .call(
            Method::POST,
            "/admin/api/users",
            Some(json!({"username": "bruno", "role": "user"})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text);
    let bruno_id = r.body["user"]["id"].as_i64().unwrap();
    let generated = r.body["password"].as_str().unwrap().to_owned();
    let r = t
        .call(
            Method::POST,
            "/admin/api/login",
            Some(json!({"username": "bruno", "password": generated})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let bruno = r.cookie.unwrap();

    let r = t
        .call(
            Method::POST,
            "/admin/api/users",
            Some(json!({"username": "Bruno", "role": "user"})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Sperren beendet die Sitzung sofort.
    let r = t
        .call(
            Method::PATCH,
            &format!("/admin/api/users/{bruno_id}"),
            Some(json!({"disabled": true})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        t.call(Method::GET, "/admin/api/me", None, Some(&bruno))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    let anna_id = t
        .call(Method::GET, "/admin/api/me", None, Some(&admin))
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    for change in [json!({"disabled": true}), json!({"role": "user"})] {
        let r = t
            .call(
                Method::PATCH,
                &format!("/admin/api/users/{anna_id}"),
                Some(change),
                Some(&admin),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    }
    let r = t
        .call(
            Method::DELETE,
            &format!("/admin/api/users/{anna_id}"),
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = t
        .call(
            Method::DELETE,
            &format!("/admin/api/users/{bruno_id}"),
            None,
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
}

fn sysinfo(id: &str, uuid: &str) -> Value {
    json!({"id": id, "uuid": uuid, "hostname": "buero-pc", "os": "Windows 11", "username": "anna", "version": "1.4.2"})
}

#[tokio::test]
async fn heartbeats_fill_the_device_list() {
    let t = TestApp::new().await;
    t.user("anna", "admin").await;
    let admin = t.login("anna").await;

    let hb = json!({"id": "123456789", "uuid": "dXVpZC1h", "ver": 1004002});
    let r = t
        .call(Method::POST, "/api/heartbeat", Some(hb.clone()), None)
        .await;
    assert_eq!(r.body["sysinfo"], true);

    let r = t
        .call(
            Method::POST,
            "/api/sysinfo",
            Some(sysinfo("123456789", "dXVpZC1h")),
            None,
        )
        .await;
    assert_eq!(r.text, "SYSINFO_UPDATED");
    let r = t.call(Method::POST, "/api/heartbeat", Some(hb), None).await;
    assert_eq!(r.body, json!({}));

    // Ein anderes Gerät mit derselben ID darf den Eintrag nicht übernehmen.
    let r = t
        .call(
            Method::POST,
            "/api/sysinfo",
            Some(sysinfo("123456789", "ZnJlbWQ=")),
            None,
        )
        .await;
    assert_eq!(r.text, "ID_NOT_FOUND");

    let r = t
        .call(Method::GET, "/admin/api/devices", None, Some(&admin))
        .await;
    let devices = r.body.as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["hostname"], "buero-pc");
    assert_eq!(devices[0]["online"], true);

    let r = t
        .call(
            Method::PATCH,
            "/admin/api/devices/123456789",
            Some(json!({"note": "Empfang"})),
            Some(&admin),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = t
        .call(Method::GET, "/admin/api/devices", None, Some(&admin))
        .await;
    assert_eq!(r.body[0]["note"], "Empfang");
}

#[tokio::test]
async fn heartbeats_are_checked_against_hbbs() {
    let hbbs_pool = db::open_memory().await.unwrap();
    sqlx::raw_sql(
        "create table peer (guid blob primary key not null, id varchar(100) not null, uuid blob not null,
           pk blob not null, created_at datetime not null default(current_timestamp), user blob,
           status tinyint, note varchar(300), info text not null) without rowid;
         insert into peer (guid, id, uuid, pk, info) values (x'01', '111', cast('uuid-a' as blob), x'00', '{\"ip\":\"203.0.113.5\"}');
         insert into peer (guid, id, uuid, pk, info) values (x'02', '222', cast('uuid-b' as blob), x'00', '{}');",
    )
    .execute(&hbbs_pool)
    .await
    .unwrap();
    let t = TestApp::with_hbbs(Some(HbbsDb::from_pool(hbbs_pool))).await;
    t.user("anna", "admin").await;
    let admin = t.login("anna").await;

    // "dXVpZC1h" ist Base64 für "uuid-a".
    let r = t
        .call(
            Method::POST,
            "/api/sysinfo",
            Some(sysinfo("111", "d3Jvbmc=")),
            None,
        )
        .await;
    assert_eq!(r.text, "ID_NOT_FOUND");
    let r = t
        .call(
            Method::POST,
            "/api/sysinfo",
            Some(sysinfo("999", "dXVpZC1h")),
            None,
        )
        .await;
    assert_eq!(r.text, "ID_NOT_FOUND");
    let r = t
        .call(
            Method::POST,
            "/api/sysinfo",
            Some(sysinfo("111", "dXVpZC1h")),
            None,
        )
        .await;
    assert_eq!(r.text, "SYSINFO_UPDATED");

    let r = t
        .call(Method::GET, "/admin/api/devices", None, Some(&admin))
        .await;
    let devices = r.body.as_array().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0]["id"], "111");
    assert_eq!(devices[0]["online"], true);
    assert_eq!(devices[0]["ip"], "203.0.113.5");
    assert_eq!(devices[0]["registered"], true);
    assert_eq!(devices[1]["id"], "222");
    assert_eq!(devices[1]["online"], false);
    assert_eq!(devices[1]["hostname"], "");
}

#[tokio::test]
async fn web_ui_is_served_with_security_headers() {
    let t = TestApp::new().await;
    let res = t
        .app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let csp = res
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("frame-ancestors 'none'"));
}
