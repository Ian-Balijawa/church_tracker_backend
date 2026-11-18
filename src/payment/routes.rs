use crate::AppState;
use axum::{
    Router,
    routing::{get, post},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/payments",
            get(crate::controller::payment::list_payments)
                .post(crate::controller::payment::create_payment),
        )
        .route(
            "/payments/{id}",
            get(crate::controller::payment::get_payment),
        )
        .route(
            "/payments/{id}/void",
            post(crate::controller::payment::void_payment),
        )
        .route(
            "/payments/preview",
            post(crate::controller::payment::preview_payment),
        )
        .route(
            "/payments/collections",
            post(crate::controller::payment::create_collection),
        )
        .route(
            "/payments/collections/{id}",
            get(crate::controller::payment::get_collection),
        )
        .route(
            "/payments/webhooks/marzpay",
            post(crate::controller::payment::marzpay_webhook),
        )
}
