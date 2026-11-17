use crate::AppState;
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects",
            get(crate::controller::project::list_projects)
                .post(crate::controller::project::create_project),
        )
        .route(
            "/projects/{id}",
            get(crate::controller::project::get_project),
        )
        .route(
            "/projects/{id}/members/{member_id}",
            axum::routing::put(crate::controller::project::attach_project_member),
        )
}
