use crate::AppState;
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/churches",
            get(crate::controller::church::list_churches)
                .post(crate::controller::church::create_church),
        )
        .route(
            "/churches/{id}",
            get(crate::controller::church::get_church)
                .patch(crate::controller::church::patch_church),
        )
}
