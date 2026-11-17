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
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use contributions_domain::{Payment as DomainPayment, replay};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn create_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<PaymentCreate>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_finance(&p)?;
    if !valid_money(body.amount, true)
        || !valid_date(body.date)
        || !matches!(body.method.as_str(), "cash" | "mobile_money" | "bank")
        || body
            .note
            .as_deref()
            .is_some_and(|s| s.chars().count() > 2000)
    {
        return Err(Problem::validation());
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
        return Err(Problem::validation());
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
    if !valid_money(body.amount, true)
        || !valid_date(body.date)
        || as_of < body.date
        || as_of > today()
    {
        return Err(Problem::validation());
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
