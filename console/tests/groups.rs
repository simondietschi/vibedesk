//! Tests der Gerätegruppen und des Zugriffsmodus.

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use vibedesk_console::{admin::insert_user, db, router, AppState, Settings};

const PASSWORD: &str = "korrekt-pferd-batterie";

struct TestApp {
    app: Router,
    state: AppState,
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

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        auth: Option<(&str, &str)>,
    ) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("x-vibedesk", "1");
        match auth {
            Some(("cookie", c)) => req = req.header(header::COOKIE, c),
            Some((_, t)) => req = req.header(header::AUTHORIZATION, format!("Bearer {t}")),
            None => {}
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
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            cookie,
        )
    }

    async fn cookie(&self, name: &str) -> String {
        let (s, _, c) = self
            .call(
                Method::POST,
                "/admin/api/login",
                Some(json!({"username": name, "password": PASSWORD})),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::OK);
        c.unwrap()
    }

    async fn token(&self, name: &str, device: &str) -> String {
        let (s, body, _) = self
            .call(
                Method::POST,
                "/api/login",
                Some(json!({"username": name, "password": PASSWORD, "id": device, "uuid": "dXVpZA==", "type": "account"})),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        body["access_token"].as_str().unwrap().to_owned()
    }

    async fn device(&self, id: &str) {
        self.call(
            Method::POST,
            "/api/sysinfo",
            Some(json!({"id": id, "uuid": "dXVpZA==", "hostname": id})),
            None,
        )
        .await;
    }
}

#[tokio::test]
async fn users_see_devices_of_their_groups() {
    let t = TestApp::new().await;
    insert_user(&t.state.db, "anna", "", "admin", PASSWORD)
        .await
        .unwrap();
    let bruno = insert_user(&t.state.db, "bruno", "", "user", PASSWORD)
        .await
        .unwrap();
    for id in ["praxis-1", "praxis-2", "privat"] {
        t.device(id).await;
    }
    let admin = t.cookie("anna").await;
    let admin = ("cookie", admin.as_str());

    let (s, group, _) = t
        .call(
            Method::POST,
            "/admin/api/device-groups",
            Some(json!({"name": "Praxis", "note": "Empfang und Labor"})),
            Some(admin),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let gid = group["id"].as_i64().unwrap();
    for id in ["praxis-1", "praxis-2"] {
        let (s, _, _) = t
            .call(
                Method::PUT,
                &format!("/admin/api/devices/{id}/group"),
                Some(json!({"group_id": gid})),
                Some(admin),
            )
            .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }
    let (_, group, _) = t
        .call(
            Method::PUT,
            &format!("/admin/api/device-groups/{gid}/users/{bruno}"),
            None,
            Some(admin),
        )
        .await;
    assert_eq!(group["device_count"], 2);
    assert_eq!(group["users"][0]["username"], "bruno");

    let token = t.token("bruno", "brunos-pc").await;
    let (_, groups, _) = t
        .call(
            Method::GET,
            "/api/device-group/accessible?current=1&pageSize=100",
            None,
            Some(("bearer", &token)),
        )
        .await;
    assert_eq!(groups, json!({"total": 1, "data": [{"name": "Praxis"}]}));
    let (_, peers, _) = t
        .call(
            Method::GET,
            "/api/peers?current=1&pageSize=100&accessible=&status=1",
            None,
            Some(("bearer", &token)),
        )
        .await;
    let ids: Vec<&str> = peers["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    // Das eigene Gerät (Anmeldung) und die zwei Geräte der Gruppe.
    assert_eq!(ids, vec!["brunos-pc", "praxis-1", "praxis-2"]);
    assert_eq!(peers["data"][1]["device_group_name"], "Praxis");

    let (_, devices, _) = t
        .call(Method::GET, "/admin/api/devices", None, Some(admin))
        .await;
    let praxis = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "praxis-1")
        .unwrap();
    assert_eq!(praxis["group"], "Praxis");

    // Gruppe löschen lässt die Geräte bestehen, nur ohne Gruppe.
    let (s, _, _) = t
        .call(
            Method::DELETE,
            &format!("/admin/api/device-groups/{gid}"),
            None,
            Some(admin),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, peers, _) = t
        .call(
            Method::GET,
            "/api/peers?current=1&pageSize=100",
            None,
            Some(("bearer", &token)),
        )
        .await;
    assert_eq!(peers["total"], 1);
}

#[tokio::test]
async fn only_admins_change_the_access_mode() {
    let t = TestApp::new().await;
    insert_user(&t.state.db, "anna", "", "admin", PASSWORD)
        .await
        .unwrap();
    insert_user(&t.state.db, "bruno", "", "user", PASSWORD)
        .await
        .unwrap();
    let admin = t.cookie("anna").await;
    let user = t.cookie("bruno").await;

    let (_, s, _) = t
        .call(
            Method::GET,
            "/admin/api/settings",
            None,
            Some(("cookie", &admin)),
        )
        .await;
    assert_eq!(s["access_mode"], "open");
    let (status, _, _) = t
        .call(
            Method::PUT,
            "/admin/api/settings",
            Some(json!({"access_mode": "alles"})),
            Some(("cookie", &admin)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = t
        .call(
            Method::PUT,
            "/admin/api/settings",
            Some(json!({"access_mode": "groups"})),
            Some(("cookie", &user)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, s, _) = t
        .call(
            Method::PUT,
            "/admin/api/settings",
            Some(json!({"access_mode": "groups"})),
            Some(("cookie", &admin)),
        )
        .await;
    assert_eq!(
        (status, s["access_mode"].as_str()),
        (StatusCode::OK, Some("groups"))
    );
    let stored: String = sqlx::query_scalar("select value from settings where key = 'access_mode'")
        .fetch_one(&t.state.db)
        .await
        .unwrap();
    assert_eq!(stored, "groups");
}
