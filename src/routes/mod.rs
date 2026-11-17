pub mod church;
pub mod contribution_auth;
pub mod health;
pub mod member;
pub mod payment;
pub mod pledge;
pub mod project;

use crate::AppState;
use axum::Router;
use utoipa_swagger_ui::SwaggerUi;

pub fn create_router() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .nest("/api/v1", contribution_auth::router())
        .nest("/api/v1", church::router())
        .nest("/api/v1", member::router())
        .nest("/api/v1", project::router())
        .nest("/api/v1", pledge::router())
        .nest("/api/v1", payment::router())
        .merge(
            SwaggerUi::new("/swagger-ui").url("/api/v1/openapi.json", crate::openapi::document()),
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn router_constructs() {
        let _ = super::create_router();
    }
}
