#![allow(dead_code)]
use axum::{
    Router,
    body::Body,
    http::{Request, header},
};
use reel_api::AppState;
use reel_auth::Device;

pub const ADMIN_PASSWORD: &str = "correct horse battery";

/// Claim the admin account and return a bearer token for it.
pub async fn admin_token(state: &AppState) -> String {
    state.auth.setup("admin", ADMIN_PASSWORD).await.unwrap();
    state
        .auth
        .login("admin", ADMIN_PASSWORD, Device::default())
        .await
        .unwrap()
        .token
}

/// Wrap `app` so every request without an `Authorization` header is sent as `token`.
pub fn with_token(app: Router, token: &str) -> Router {
    let token = format!("Bearer {token}");
    app.layer(axum::middleware::map_request(
        move |mut req: Request<Body>| {
            let token = token.clone();
            async move {
                if !req.headers().contains_key(header::AUTHORIZATION) {
                    req.headers_mut()
                        .insert(header::AUTHORIZATION, token.parse().unwrap());
                }
                req
            }
        },
    ))
}

/// Set up the admin account and return `app` authenticated as admin.
pub async fn authed(state: &AppState, app: Router) -> Router {
    let t = admin_token(state).await;
    with_token(app, &t)
}
