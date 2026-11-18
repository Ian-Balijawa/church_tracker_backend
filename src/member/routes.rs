use crate::AppState;
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/members",
            get(crate::controller::member::list_members)
                .post(crate::controller::member::create_member),
        )
        .route("/members/{id}", get(crate::controller::member::get_member))
}
