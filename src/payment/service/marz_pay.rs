use crate::{AppState, http::contribution::Problem};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

fn credentials(state: &AppState) -> Result<(&str, &str), Problem> {
    match (
        state.config.marz_pay_api_key.as_deref(),
        state.config.marz_pay_api_secret.as_deref(),
    ) {
        (Some(key), Some(secret)) if !key.trim().is_empty() && !secret.trim().is_empty() => {
            Ok((key, secret))
        }
        _ => Err(Problem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "payment_provider_unconfigured",
            "Mobile money payments are not configured. Contact the administrator.",
        )),
    }
}

pub(crate) fn ensure_configured(state: &AppState) -> Result<(), Problem> {
    credentials(state).map(|_| ())
}

pub(crate) async fn create_collection(
    state: &AppState,
    amount: i64,
    phone_number: &str,
    reference: Uuid,
    description: Option<&str>,
    callback_url: &str,
) -> Result<Value, Problem> {
    let (key, secret) = credentials(state)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| provider_unavailable())?;
    let mut body = json!({
        "amount": amount,
        "phone_number": phone_number,
        "country": "UG",
        "method": "mobile_money",
        "reference": reference.to_string(),
        "callback_url": callback_url,
    });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    let response = client
        .post(format!(
            "{}/collect-money",
            state.config.marz_pay_base_url.trim_end_matches('/')
        ))
        .basic_auth(key, Some(secret))
        .json(&body)
        .send()
        .await
        .map_err(|_| provider_unavailable())?;
    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), %reference, "MarzPay rejected collection request");
        return Err(provider_unavailable());
    }
    let result: Value = response.json().await.map_err(|_| provider_unavailable())?;
    let transaction = &result["data"]["transaction"];
    let expected_reference = reference.to_string();
    let valid_uuid = transaction["uuid"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .is_some();
    if result["status"].as_str() != Some("success")
        || transaction["reference"].as_str() != Some(expected_reference.as_str())
        || !valid_uuid
    {
        tracing::warn!(%reference, "MarzPay returned an invalid collection response");
        return Err(provider_unavailable());
    }
    Ok(result)
}

pub(crate) async fn collection_details(
    state: &AppState,
    external_uuid: &str,
) -> Result<Value, Problem> {
    let (key, secret) = credentials(state)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| provider_unavailable())?;
    let response = client
        .get(format!(
            "{}/collect-money/{}",
            state.config.marz_pay_base_url.trim_end_matches('/'),
            external_uuid
        ))
        .basic_auth(key, Some(secret))
        .send()
        .await
        .map_err(|_| provider_unavailable())?;
    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "MarzPay collection status lookup failed");
        return Err(provider_unavailable());
    }
    response.json().await.map_err(|_| provider_unavailable())
}

fn provider_unavailable() -> Problem {
    Problem::new(
        StatusCode::BAD_GATEWAY,
        "payment_provider_unavailable",
        "The payment provider could not complete the request. Check the collection status before retrying.",
    )
}
