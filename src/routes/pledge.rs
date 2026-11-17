use crate::AppState;
use axum::{
    Router,
    routing::{get, post},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/pledges",
            get(crate::controller::pledge::list_pledges)
                .post(crate::controller::pledge::create_pledge),
        )
        .route("/pledges/{id}", get(crate::controller::pledge::get_pledge))
        .route(
            "/pledges/{id}/events",
            get(crate::controller::pledge::pledge_events),
        )
        .route(
            "/pledges/{id}/pause",
            post(crate::controller::pledge::pause_pledge),
        )
        .route(
            "/pledges/{id}/resume",
            post(crate::controller::pledge::resume_pledge),
        )
        .route(
            "/pledges/{id}/cancel",
            post(crate::controller::pledge::cancel_pledge),
        )
        .route(
            "/pledges/{id}/versions",
            post(crate::controller::pledge::add_pledge_version),
        )
}
