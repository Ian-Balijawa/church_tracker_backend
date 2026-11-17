use crate::{AppState, http::contribution::Problem};
use axum::{Json, extract::State, http::StatusCode};

pub(crate) async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok"}))
}

pub(crate) async fn ready_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, Problem> {
    sqlx::query("SELECT 1")
        .execute(&state.contributions_db)
        .await
        .map_err(|_| {
            Problem::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "Service unavailable.",
            )
        })?;
    let mut redis = state.redis.clone();
    let _: String = redis::cmd("PING")
        .query_async(&mut redis)
        .await
        .map_err(|_| {
            Problem::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "Service unavailable.",
            )
        })?;
    Ok(Json(serde_json::json!({"status":"ok"})))
}
