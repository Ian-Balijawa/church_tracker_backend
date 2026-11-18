use crate::AppState;
use crate::http::contribution::{
    ContractJson, ContractQuery, Problem, etag, hash, if_match, item, list_response, no_store,
    req_key,
};
use crate::model::contribution::*;
use crate::model::payment::*;
use crate::repository::contribution::*;
use crate::service::contribution::*;
use crate::service::contribution_auth::{
    member_access, principal, project_access, require_finance, write_principal,
};
use crate::service::payment::*;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use contributions_domain::{Payment as DomainPayment, replay};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

fn normalized_collection_phone(raw: &str) -> Option<(String, &'static str)> {
    let mut digits = raw.chars().filter(char::is_ascii_digit).collect::<String>();
    if digits.starts_with("00") {
        digits.drain(..2);
    }
    if digits.starts_with("2560") {
        digits.replace_range(3..4, "");
    } else if let Some(local) = digits.strip_prefix('0') {
        digits = format!("256{local}");
    } else if digits.starts_with('7') || digits.starts_with("31") || digits.starts_with("39") {
        digits.insert_str(0, "256");
    }
    if digits.len() != 12 || !digits.starts_with("256") {
        return None;
    }
    let prefix = &digits[3..6];
    let prefix = prefix.parse::<u16>().ok()?;
    let provider = if (760..=789).contains(&prefix)
        || (310..=319).contains(&prefix)
        || (390..=399).contains(&prefix)
    {
        "mtn"
    } else if (700..=705).contains(&prefix) || (750..=759).contains(&prefix) {
        "airtel"
    } else {
        return None;
    };
    Some((format!("+{digits}"), provider))
}

pub(crate) async fn create_collection(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<CreateCollectionBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_finance(&p)?;
    if !(500..=10_000_000).contains(&body.amount) {
        return Err(Problem::field_validation(
            "/amount",
            "invalid_amount",
            "Amount must be between 500 and 10,000,000 UGX.",
        ));
    }
    let (phone, provider) = normalized_collection_phone(&body.phone).ok_or_else(|| {
        Problem::field_validation(
            "/phone",
            "invalid_phone",
            "Enter a valid Ugandan MTN or Airtel mobile number.",
        )
    })?;
    if body
        .description
        .as_deref()
        .is_some_and(|s| s.trim().is_empty() || s.chars().count() > 255)
    {
        return Err(Problem::field_validation(
            "/description",
            "invalid_description",
            "Description must contain 1 to 255 characters.",
        ));
    }
    full_finance_access(&state.contributions_db, &p, body.member_id).await?;
    if let Some(project_id) = body.project_id
        && !project_access(&state.contributions_db, &p, project_id).await?
    {
        return Err(Problem::not_found());
    }
    let callback_base = state
        .config
        .public_api_url
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            Problem::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "payment_provider_unconfigured",
                "Mobile money payments are not configured. Contact the administrator.",
            )
        })?;
    crate::service::marz_pay::ensure_configured(&state)?;
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /payments/collections {}",
        json!({"memberId":body.member_id,"projectId":body.project_id,"amount":body.amount,"phone":phone,"description":body.description})
    ));
    if let Some(row) = sqlx::query("SELECT id,request_fingerprint FROM payment_collections WHERE organization_id=$1 AND initiated_by=$2 AND idempotency_key=$3")
        .bind(p.organization_id).bind(p.id).bind(key).fetch_optional(&state.contributions_db).await.map_err(db_error)? {
        if row.get::<String,_>("request_fingerprint") != fingerprint { return Err(Problem::conflict()); }
        let id: Uuid = row.get("id");
        let value = collection_json(&state, &p, id).await?;
        return Ok(item(value, StatusCode::OK, None, None));
    }
    let id = Uuid::new_v4();
    let reference = Uuid::new_v4();
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    let member_status =
        crate::repository::member::status_for_update(&mut tx, p.organization_id, body.member_id)
            .await
            .map_err(db_error)?
            .ok_or_else(Problem::not_found)?;
    if member_status != "active" {
        return Err(Problem::conflict());
    }
    let inserted = sqlx::query("INSERT INTO payment_collections(id,reference,organization_id,initiated_by,member_id,project_id,amount,phone_number,provider,description,request_fingerprint,idempotency_key) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT (organization_id,initiated_by,idempotency_key) DO NOTHING RETURNING id")
        .bind(id).bind(reference).bind(p.organization_id).bind(p.id).bind(body.member_id).bind(body.project_id).bind(body.amount).bind(&phone).bind(provider).bind(body.description.as_deref().map(str::trim)).bind(&fingerprint).bind(key).execute(&mut *tx).await.map_err(db_error)?;
    if inserted.rows_affected() == 0 {
        tx.rollback().await.map_err(db_error)?;
        let row = sqlx::query("SELECT id,request_fingerprint FROM payment_collections WHERE organization_id=$1 AND initiated_by=$2 AND idempotency_key=$3")
            .bind(p.organization_id).bind(p.id).bind(key).fetch_one(&state.contributions_db).await.map_err(db_error)?;
        if row.get::<String, _>("request_fingerprint") != fingerprint {
            return Err(Problem::conflict());
        }
        let existing_id: Uuid = row.get("id");
        return Ok(item(
            collection_json(&state, &p, existing_id).await?,
            StatusCode::OK,
            None,
            None,
        ));
    }
    tx.commit().await.map_err(db_error)?;

    let callback = format!(
        "{}/api/v1/payments/webhooks/marzpay",
        callback_base.trim_end_matches('/')
    );
    let result = crate::service::marz_pay::create_collection(
        &state,
        body.amount,
        &phone,
        reference,
        body.description.as_deref(),
        &callback,
    )
    .await;
    match result {
        Ok(response) => {
            let transaction = &response["data"]["transaction"];
            let collection = &response["data"]["collection"];
            let external_uuid = transaction["uuid"]
                .as_str()
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(|value| value.to_string())
                .ok_or_else(|| {
                    Problem::new(
                        StatusCode::BAD_GATEWAY,
                        "payment_provider_unavailable",
                        "The payment provider returned an invalid response.",
                    )
                })?;
            let provider_phone = collection["phone_number"].as_str();
            let returned_provider = collection["provider"].as_str();
            let returned_amount = collection["amount"]["raw"].as_i64();
            let currency = collection["amount"]["currency"].as_str();
            if provider_phone
                .and_then(normalized_collection_phone)
                .is_none_or(|(value, _)| value != phone)
                || returned_provider.is_none_or(|value| value.to_ascii_lowercase() != provider)
                || returned_amount != Some(body.amount)
                || currency != Some("UGX")
            {
                return Err(Problem::new(
                    StatusCode::BAD_GATEWAY,
                    "payment_provider_unavailable",
                    "The payment provider returned an invalid collection response.",
                ));
            }
            let mode = collection["mode"].as_str();
            let status = if mode == Some("sandbox") {
                "sandbox"
            } else {
                normalize_provider_status(transaction["status"].as_str().unwrap_or("pending"))
            };
            let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
            sqlx::query("UPDATE payment_collections SET external_uuid=$2,status=CASE WHEN status IN ('successful','failed','cancelled','sandbox') THEN status ELSE $3 END,mode=$4,provider_reference=$5,provider_response=$6,updated_at=now() WHERE id=$1")
                .bind(id).bind(external_uuid).bind(status).bind(collection["mode"].as_str()).bind(transaction["provider_reference"].as_str()).bind(response).execute(&mut *tx).await.map_err(db_error)?;
            tx.commit().await.map_err(db_error)?;
            let value = collection_json(&state, &p, id).await?;
            Ok(item(
                value,
                StatusCode::CREATED,
                None,
                Some(format!("/api/v1/payments/collections/{id}")),
            ))
        }
        Err(problem) => {
            // The provider may have accepted the request before the connection failed.
            // Keep it initiating so an idempotent retry cannot create a second charge.
            Err(problem)
        }
    }
}

fn normalize_provider_status(status: &str) -> &'static str {
    match status.to_ascii_lowercase().as_str() {
        "successful" | "success" | "completed" => "successful",
        "failed" | "failure" => "failed",
        "cancelled" | "canceled" => "cancelled",
        "sandbox" => "sandbox",
        "processing" => "processing",
        _ => "pending",
    }
}

async fn collection_json(
    state: &AppState,
    p: &crate::service::contribution_auth::Principal,
    id: Uuid,
) -> Result<serde_json::Value, Problem> {
    let row = sqlx::query("SELECT id,reference,member_id,project_id,amount,currency,phone_number,provider,status,description,provider_transaction_id,failure_reason,initiated_at,completed_at,created_at,updated_at FROM payment_collections WHERE id=$1 AND organization_id=$2")
        .bind(id).bind(p.organization_id).fetch_optional(&state.contributions_db).await.map_err(db_error)?.ok_or_else(Problem::not_found)?;
    let member: Uuid = row.get("member_id");
    full_finance_access(&state.contributions_db, p, member).await?;
    Ok(
        json!({"id":id,"reference":row.get::<Uuid,_>("reference"),"memberId":member,"projectId":row.get::<Option<Uuid>,_>("project_id"),"amount":row.get::<i64,_>("amount"),"currency":row.get::<String,_>("currency"),"phone":row.get::<String,_>("phone_number"),"provider":row.get::<String,_>("provider"),"status":row.get::<String,_>("status"),"description":row.get::<Option<String>,_>("description"),"providerTransactionId":row.get::<Option<String>,_>("provider_transaction_id"),"failureReason":row.get::<Option<String>,_>("failure_reason"),"initiatedAt":row.get::<chrono::DateTime<chrono::Utc>,_>("initiated_at"),"completedAt":row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("completed_at"),"createdAt":row.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),"updatedAt":row.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")}),
    )
}

pub(crate) async fn get_collection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let value = collection_json(&state, &p, id).await?;
    Ok(item(value, StatusCode::OK, None, None))
}

pub(crate) async fn marzpay_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Problem> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        )
    })?;
    if let Some(secret) = state
        .config
        .marz_pay_webhook_secret
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let timestamp = headers
            .get("x-marzpay-timestamp")
            .and_then(|x| x.to_str().ok())
            .ok_or_else(|| {
                Problem::new(
                    StatusCode::UNAUTHORIZED,
                    "invalid_webhook_signature",
                    "Payment callback signature is invalid.",
                )
            })?;
        let signature = headers
            .get("x-marzpay-signature")
            .and_then(|x| x.to_str().ok())
            .unwrap_or("");
        let sig = signature
            .split(',')
            .find_map(|part| part.trim().strip_prefix("v1="))
            .or_else(|| signature.strip_prefix("v1="))
            .ok_or_else(|| {
                Problem::new(
                    StatusCode::UNAUTHORIZED,
                    "invalid_webhook_signature",
                    "Payment callback signature is invalid.",
                )
            })?;
        let epoch = timestamp.parse::<i64>().map_err(|_| {
            Problem::new(
                StatusCode::UNAUTHORIZED,
                "invalid_webhook_signature",
                "Payment callback signature is invalid.",
            )
        })?;
        if chrono::Utc::now().timestamp().abs_diff(epoch) > 300 {
            return Err(Problem::new(
                StatusCode::UNAUTHORIZED,
                "invalid_webhook_signature",
                "Payment callback signature is invalid.",
            ));
        }
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(Problem::internal)?;
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(&body);
        if sig.len() % 2 != 0 || !sig.is_ascii() {
            return Err(Problem::new(
                StatusCode::UNAUTHORIZED,
                "invalid_webhook_signature",
                "Payment callback signature is invalid.",
            ));
        }
        let signature_bytes = (0..sig.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&sig[i..i + 2], 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                Problem::new(
                    StatusCode::UNAUTHORIZED,
                    "invalid_webhook_signature",
                    "Payment callback signature is invalid.",
                )
            })?;
        mac.verify_slice(&signature_bytes).map_err(|_| {
            Problem::new(
                StatusCode::UNAUTHORIZED,
                "invalid_webhook_signature",
                "Payment callback signature is invalid.",
            )
        })?;
    }
    let data = payload.get("data").unwrap_or(&payload);
    let transaction = data.get("transaction").unwrap_or(data);
    if transaction
        .get("type")
        .and_then(|v| v.as_str())
        .is_some_and(|kind| kind != "collection")
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        ));
    }
    let reference = transaction
        .get("reference")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "invalid_webhook",
                "Invalid payment callback.",
            )
        })?;
    let external_uuid = transaction
        .get("uuid")
        .and_then(|v| v.as_str())
        .and_then(|v| Uuid::parse_str(v).ok())
        .map(|v| v.to_string())
        .ok_or_else(|| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "invalid_webhook",
                "Invalid payment callback.",
            )
        })?;
    let incoming_status = normalize_provider_status(
        transaction
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending"),
    );
    let event = payload
        .get("event_type")
        .or_else(|| payload.get("event"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let callback_status = match event {
        "collection.completed" | "collection.success" | "collection.successful" => "successful",
        "collection.failed" => "failed",
        _ => incoming_status,
    };
    let row = sqlx::query("SELECT id,organization_id,initiated_by,member_id,project_id,amount,phone_number,provider,status,mode,external_uuid FROM payment_collections WHERE reference=$1")
        .bind(reference).fetch_optional(&state.contributions_db).await.map_err(db_error)?.ok_or_else(Problem::not_found)?;
    if row
        .get::<Option<String>, _>("external_uuid")
        .as_deref()
        .is_some_and(|stored| stored != external_uuid)
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        ));
    }
    let authoritative = if state
        .config
        .marz_pay_webhook_secret
        .as_deref()
        .is_none_or(|s| s.trim().is_empty())
    {
        Some(crate::service::marz_pay::collection_details(&state, &external_uuid).await?)
    } else {
        None
    };
    let verified_root = authoritative
        .as_ref()
        .map(|v| v.get("data").unwrap_or(v))
        .unwrap_or(data);
    let verified = authoritative
        .as_ref()
        .map(|v| {
            v.get("data")
                .unwrap_or(v)
                .get("transaction")
                .unwrap_or(v.get("data").unwrap_or(v))
        })
        .unwrap_or(transaction);
    let verified_uuid = verified
        .get("uuid")
        .and_then(|v| v.as_str())
        .unwrap_or(&external_uuid);
    let verified_reference = verified
        .get("reference")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if verified_uuid != external_uuid || verified_reference != reference.to_string() {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        ));
    }
    let collection_meta = verified_root
        .get("collection")
        .or_else(|| verified_root.get("details"))
        .or_else(|| data.get("collection"))
        .or_else(|| data.get("details"));
    let amount_value = verified
        .get("amount")
        .or_else(|| collection_meta.and_then(|v| v.get("amount")));
    let verified_amount = amount_value.and_then(|v| v.as_i64()).or_else(|| {
        amount_value
            .and_then(|v| v.get("raw"))
            .and_then(|v| v.as_i64())
    });
    if verified_amount != Some(row.get::<i64, _>("amount")) {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        ));
    }
    let currency = amount_value
        .and_then(|v| v.get("currency"))
        .and_then(|v| v.as_str())
        .or_else(|| verified.get("currency").and_then(|v| v.as_str()))
        .or_else(|| {
            collection_meta
                .and_then(|v| v.get("amount"))
                .and_then(|v| v.get("currency"))
                .and_then(|v| v.as_str())
        });
    if currency != Some("UGX") {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        ));
    }
    let returned_phone = collection_meta
        .and_then(|v| v.get("phone_number"))
        .and_then(|v| v.as_str())
        .or_else(|| verified.get("phone_number").and_then(|v| v.as_str()));
    if returned_phone.is_none_or(|value| {
        normalized_collection_phone(value)
            .is_none_or(|(phone, _)| phone != row.get::<String, _>("phone_number"))
    }) {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        ));
    }
    let returned_provider = collection_meta
        .and_then(|v| v.get("provider"))
        .and_then(|v| v.as_str())
        .or_else(|| verified.get("provider").and_then(|v| v.as_str()));
    if returned_provider
        .is_none_or(|value| value.to_ascii_lowercase() != row.get::<String, _>("provider"))
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid_webhook",
            "Invalid payment callback.",
        ));
    }
    let new_status = if row.get::<Option<String>, _>("mode").as_deref() == Some("sandbox") {
        "sandbox"
    } else if authoritative.is_some() {
        normalize_provider_status(
            verified
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending"),
        )
    } else {
        callback_status
    };
    let id: Uuid = row.get("id");
    let org: Uuid = row.get("organization_id");
    let member: Uuid = row.get("member_id");
    let project: Option<Uuid> = row.get("project_id");
    let amount: i64 = row.get("amount");
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    let previous = row.get::<String, _>("status");
    if matches!(
        previous.as_str(),
        "successful" | "failed" | "cancelled" | "sandbox"
    ) && previous != new_status
    {
        tx.rollback().await.map_err(db_error)?;
        return Ok((StatusCode::OK, Json(json!({"received":true}))).into_response());
    }
    sqlx::query("UPDATE payment_collections SET external_uuid=COALESCE(external_uuid,$2),status=CASE WHEN status IN ('successful','failed','cancelled','sandbox') THEN status ELSE $3 END,provider_transaction_id=COALESCE($4,provider_transaction_id),failure_reason=CASE WHEN $3='failed' THEN 'Mobile money collection failed' ELSE NULL END,webhook_payload=$5,webhook_attempts=webhook_attempts+1,last_webhook_at=now(),completed_at=CASE WHEN $3 IN ('successful','failed','cancelled') THEN COALESCE(completed_at,now()) ELSE completed_at END,updated_at=now() WHERE id=$1")
        .bind(id).bind(external_uuid).bind(new_status).bind(verified.get("provider_reference").and_then(|v|v.as_str()).or_else(||collection_meta.and_then(|v|v.get("provider_transaction_id")).and_then(|v|v.as_str()))).bind(payload).execute(&mut *tx).await.map_err(db_error)?;
    if new_status == "successful" {
        let payment_id = id;
        let inserted = sqlx::query("INSERT INTO payments(id,organization_id,member_id,project_id,amount,date,method,note,collection_id) VALUES($1,$2,$3,$4,$5,$6,'mobile_money',$7,$1) ON CONFLICT(collection_id) DO NOTHING")
            .bind(payment_id).bind(org).bind(member).bind(project).bind(amount).bind(today()).bind("MarzPay mobile money collection").execute(&mut *tx).await.map_err(db_error)?;
        if inserted.rows_affected() > 0 {
            let calc = recalculate(&mut tx, org, member, Some(payment_id)).await?;
            let actor = sqlx::query("SELECT id,organization_id,role FROM users WHERE id=$1")
                .bind(row.get::<Uuid, _>("initiated_by"))
                .fetch_one(&mut *tx)
                .await
                .map_err(db_error)?;
            let p = crate::service::contribution_auth::Principal {
                id: actor.get("id"),
                organization_id: actor.get("organization_id"),
                role: actor.get("role"),
                session_id: Uuid::nil(),
                csrf_token: String::new(),
            };
            audit(&mut tx, &p, "create", "payment", payment_id, None).await?;
            let _ = calc;
        }
    }
    tx.commit().await.map_err(db_error)?;
    Ok((StatusCode::OK, Json(json!({"received":true}))).into_response())
}

pub(crate) async fn create_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<PaymentCreate>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_finance(&p)?;
    if !valid_money(body.amount, true) {
        return Err(Problem::field_validation(
            "/amount",
            "invalid_amount",
            "Amount must be a positive whole number of shillings.",
        ));
    }
    if !valid_date(body.date) {
        return Err(Problem::field_validation(
            "/date",
            "invalid_date",
            "Payment date cannot be in the future.",
        ));
    }
    if !matches!(body.method.as_str(), "cash" | "mobile_money" | "bank") {
        return Err(Problem::field_validation(
            "/method",
            "invalid_method",
            "Method must be cash, mobile_money, or bank.",
        ));
    }
    if body
        .note
        .as_deref()
        .is_some_and(|note| note.chars().count() > 2000)
    {
        return Err(Problem::field_validation(
            "/note",
            "too_long",
            "Note cannot exceed 2000 characters.",
        ));
    }
    full_finance_access(&state.contributions_db, &p, body.member_id).await?;
    if let Some(id) = body.project_id
        && !project_access(&state.contributions_db, &p, id).await?
    {
        return Err(Problem::forbidden());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /payments {}",
        json!({"id":body.id,"memberId":body.member_id,"projectId":body.project_id,"amount":body.amount,"date":body.date,"method":body.method,"note":body.note})
    ));
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    let member_status =
        crate::repository::member::status_for_update(&mut tx, p.organization_id, body.member_id)
            .await
            .map_err(db_error)?
            .ok_or_else(Problem::not_found)?;
    if member_status != "active" {
        return Err(Problem::conflict());
    }
    sqlx::query("INSERT INTO payments(id,organization_id,member_id,project_id,amount,date,method,note) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(body.id).bind(p.organization_id).bind(body.member_id).bind(body.project_id).bind(body.amount).bind(body.date).bind(&body.method).bind(&body.note).execute(&mut *tx).await.map_err(db_error)?;
    let calc = recalculate(&mut tx, p.organization_id, body.member_id, Some(body.id)).await?;
    audit(&mut tx, &p, "create", "payment", body.id, None).await?;
    let resource = payment_json_tx(
        &mut tx,
        body.id,
        calc.payment_credit.get(&body.id).copied().unwrap_or(0),
    )
    .await?;
    let summary = json!({"asOf":today(),"debt":calc.balance.debt,"dueToday":calc.balance.due_today,"advance":calc.balance.advance,"unallocatedCredit":calc.balance.unallocated_credit});
    let envelope = json!({"data":{"payment":resource,"memberSummary":summary}});
    let tag = etag(body.id, 1);
    let location = format!("/api/v1/payments/{}", body.id);
    save_replay(
        &mut tx,
        &p,
        key,
        StatusCode::CREATED,
        &envelope,
        Some(&location),
        Some(&tag),
    )
    .await?;
    tx.commit().await.map_err(db_error)?;
    Ok(no_store(
        (
            StatusCode::CREATED,
            [
                (axum::http::header::LOCATION, location),
                (axum::http::header::ETAG, tag),
            ],
            Json(envelope),
        )
            .into_response(),
    ))
}

pub(crate) async fn get_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let value = payment_json(&state.contributions_db, &p, id).await?;
    Ok(item(
        &value,
        StatusCode::OK,
        Some(etag(id, value["version"].as_i64().unwrap_or(1))),
        None,
    ))
}

pub(crate) async fn list_payments(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let (limit, offset) = query.validate(&["date", "amount", "createdAt"])?;
    let rows=sqlx::query("SELECT id,member_id,project_id FROM payments WHERE organization_id=$1 ORDER BY date DESC,id")
        .bind(p.organization_id).fetch_all(&state.contributions_db).await.map_err(db_error)?;
    let mut ids = Vec::new();
    for row in rows {
        let member: Uuid = row.get("member_id");
        let project: Option<Uuid> = row.get("project_id");
        if member_access(&state.contributions_db, &p, member).await?
            && match project {
                Some(id) => project_access(&state.contributions_db, &p, id).await?,
                None => full_finance_access(&state.contributions_db, &p, member)
                    .await
                    .is_ok(),
            }
        {
            ids.push(row.get::<Uuid, _>("id"));
        }
    }
    let total = ids.len() as i64;
    let mut data = Vec::new();
    for id in ids.into_iter().skip(offset as usize).take(limit as usize) {
        data.push(payment_json(&state.contributions_db, &p, id).await?);
    }
    Ok(list_response(data, query.meta(total)))
}

pub(crate) async fn void_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractJson(body): ContractJson<VoidBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_finance(&p)?;
    if !valid_name(&body.reason) {
        return Err(Problem::field_validation(
            "/reason",
            "invalid_reason",
            "Enter a reason between 1 and 200 characters.",
        ));
    }
    let (member_id, project_id) =
        crate::repository::payment::owner(&state.contributions_db, p.organization_id, id)
            .await
            .map_err(db_error)?
            .ok_or_else(Problem::not_found)?;
    full_finance_access(&state.contributions_db, &p, member_id).await?;
    if let Some(project_id) = project_id
        && !project_access(&state.contributions_db, &p, project_id).await?
    {
        return Err(Problem::forbidden());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /payments/{id}/void {} {}",
        headers
            .get("if-match")
            .and_then(|h| h.to_str().ok())
            .unwrap_or(""),
        body.reason
    ));
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    sqlx::query("SELECT id FROM members WHERE id=$1 FOR UPDATE")
        .bind(member_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    let row = sqlx::query("SELECT status,version FROM payments WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    let version: i64 = row.get("version");
    if_match(&headers, id, version)?;
    if row.get::<&str, _>("status") != "recorded" {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "invalid_state",
            "Payment is already voided.",
        ));
    }
    sqlx::query("UPDATE payments SET status='voided',void_reason=$2,voided_at=now(),voided_by=$3,version=version+1,updated_at=now() WHERE id=$1")
        .bind(id).bind(body.reason.trim()).bind(p.id).execute(&mut *tx).await.map_err(db_error)?;
    let calc = recalculate(&mut tx, p.organization_id, member_id, Some(id)).await?;
    audit(&mut tx, &p, "void", "payment", id, Some(body.reason.trim())).await?;
    let resource = payment_json_tx(&mut tx, id, 0).await?;
    let summary = json!({"asOf":today(),"debt":calc.balance.debt,"dueToday":calc.balance.due_today,"advance":calc.balance.advance,"unallocatedCredit":calc.balance.unallocated_credit});
    let envelope = json!({"data":{"payment":resource,"memberSummary":summary}});
    let tag = etag(id, version + 1);
    save_replay(
        &mut tx,
        &p,
        key,
        StatusCode::OK,
        &envelope,
        None,
        Some(&tag),
    )
    .await?;
    tx.commit().await.map_err(db_error)?;
    Ok(no_store(
        (
            StatusCode::OK,
            [(axum::http::header::ETAG, tag)],
            Json(envelope),
        )
            .into_response(),
    ))
}

pub(crate) async fn preview_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<PreviewBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_finance(&p)?;
    let as_of = body.as_of.unwrap_or_else(today);
    if !valid_money(body.amount, true) {
        return Err(Problem::field_validation(
            "/amount",
            "invalid_amount",
            "Amount must be a positive whole number of shillings.",
        ));
    }
    if !valid_date(body.date) {
        return Err(Problem::field_validation(
            "/date",
            "invalid_date",
            "Payment date cannot be in the future.",
        ));
    }
    if as_of < body.date || as_of > today() {
        return Err(Problem::field_validation(
            "/asOf",
            "invalid_as_of_date",
            "asOf must be between the payment date and today.",
        ));
    }
    full_finance_access(&state.contributions_db, &p, body.member_id).await?;
    if let Some(project_id) = body.project_id
        && !project_access(&state.contributions_db, &p, project_id).await?
    {
        return Err(Problem::forbidden());
    }
    let (pledges, mut payments) =
        load_ledger(&state.contributions_db, p.organization_id, body.member_id).await?;
    let before = replay(&pledges, &payments, as_of).map_err(Problem::internal)?;
    payments.push(DomainPayment {
        id: body.id,
        project_id: body.project_id,
        amount: body.amount,
        date: body.date,
        voided: false,
    });
    let after = replay(&pledges, &payments, as_of).map_err(Problem::internal)?;
    let allocations = after
        .allocations
        .into_iter()
        .filter(|a| a.payment_id == body.id)
        .collect::<Vec<_>>();
    let ledger_version: i64 = sqlx::query_scalar("SELECT version FROM members WHERE id=$1")
        .bind(body.member_id)
        .fetch_one(&state.contributions_db)
        .await
        .map_err(db_error)?;
    Ok(item(
        json!({"allocations":allocations,"unallocatedCredit":after.payment_credit.get(&body.id).copied().unwrap_or(0),"before":{"debt":before.balance.debt,"dueToday":before.balance.due_today,"advance":before.balance.advance},"after":{"debt":after.balance.debt,"dueToday":after.balance.due_today,"advance":after.balance.advance},"asOf":as_of,"ledgerVersion":ledger_version}),
        StatusCode::OK,
        None,
        None,
    ))
}
