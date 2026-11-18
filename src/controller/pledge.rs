use crate::AppState;
use crate::http::contribution::{
    ContractJson, ContractQuery, Problem, etag, hash, if_match, item, list_response, req_key,
};
use crate::model::contribution::*;
use crate::model::pledge::*;
use crate::repository::contribution::*;
use crate::service::contribution::*;
use crate::service::contribution_auth::{
    member_access, principal, project_access, require_finance, write_principal,
};
use crate::service::{payment::*, pledge::*};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn create_pledge(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<PledgeCreate>,
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
    if body.end_date.is_some_and(|end| end < body.start_date) {
        return Err(Problem::field_validation(
            "/endDate",
            "invalid_date_range",
            "End date must be the same as or later than start date.",
        ));
    }
    parse_frequency(&body.frequency)?;
    full_finance_access(&state.contributions_db, &p, body.member_id).await?;
    if !project_access(&state.contributions_db, &p, body.project_id).await? {
        return Err(Problem::forbidden());
    }
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project_members WHERE project_id=$1 AND member_id=$2 AND detached_at IS NULL)")
        .bind(body.project_id).bind(body.member_id).fetch_one(&state.contributions_db).await.map_err(db_error)?;
    if !active {
        return Err(Problem::conflict());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /pledges {}",
        json!({"id":body.id,"memberId":body.member_id,"projectId":body.project_id,"amount":body.amount,"frequency":body.frequency,"startDate":body.start_date,"endDate":body.end_date})
    ));
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    sqlx::query("SELECT id FROM members WHERE id=$1 FOR UPDATE")
        .bind(body.member_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    sqlx::query("INSERT INTO pledges(id,organization_id,member_id,project_id,frequency,start_date,end_date) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(body.id).bind(p.organization_id).bind(body.member_id).bind(body.project_id).bind(&body.frequency).bind(body.start_date).bind(body.end_date).execute(&mut *tx).await.map_err(db_error)?;
    let version_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pledge_versions(id,pledge_id,amount,effective_from) VALUES($1,$2,$3,$4)",
    )
    .bind(version_id)
    .bind(body.id)
    .bind(body.amount)
    .bind(body.start_date)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    let calc = recalculate(&mut tx, p.organization_id, body.member_id, None).await?;
    audit(&mut tx, &p, "create", "pledge", body.id, None).await?;
    let b = calc
        .pledge_balances
        .get(&body.id)
        .cloned()
        .unwrap_or_default();
    let resource = json!({"id":body.id,"memberId":body.member_id,"projectId":body.project_id,"frequency":body.frequency,"startDate":body.start_date,"endDate":body.end_date,"status":"active","versions":[{"id":version_id,"amount":body.amount,"effectiveFrom":body.start_date,"createdAt":Utc::now()}],"demand":b.demand,"paid":b.paid,"debt":b.debt,"dueToday":b.due_today,"advance":b.advance,"version":1,"createdAt":Utc::now(),"updatedAt":Utc::now()});
    let envelope = json!({"data":resource});
    let tag = etag(body.id, 1);
    let location = format!("/api/v1/pledges/{}", body.id);
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
    Ok(item(
        resource,
        StatusCode::CREATED,
        Some(tag),
        Some(location),
    ))
}

pub(crate) async fn get_pledge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    query.validate(&["startDate"])?;
    let value = pledge_json(
        &state.contributions_db,
        &p,
        id,
        query.as_of.unwrap_or_else(today),
    )
    .await?;
    Ok(item(
        &value,
        StatusCode::OK,
        Some(etag(id, value["version"].as_i64().unwrap_or(1))),
        None,
    ))
}

pub(crate) async fn list_pledges(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let (limit, offset) = query.validate(&["startDate", "createdAt"])?;
    let rows=sqlx::query("SELECT id,member_id,project_id FROM pledges WHERE organization_id=$1 ORDER BY start_date,id").bind(p.organization_id).fetch_all(&state.contributions_db).await.map_err(db_error)?;
    let mut authorized = Vec::new();
    for row in rows {
        if member_access(&state.contributions_db, &p, row.get("member_id")).await?
            && project_access(&state.contributions_db, &p, row.get("project_id")).await?
        {
            authorized.push(row.get::<Uuid, _>("id"));
        }
    }
    let total = authorized.len() as i64;
    let mut data = Vec::new();
    for id in authorized
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
    {
        data.push(
            pledge_json(
                &state.contributions_db,
                &p,
                id,
                query.as_of.unwrap_or_else(today),
            )
            .await?,
        );
    }
    Ok(list_response(data, query.meta(total)))
}

async fn transition_pledge(
    state: AppState,
    headers: HeaderMap,
    id: Uuid,
    body: TransitionBody,
    action: &'static str,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_finance(&p)?;
    if body.effective_date > today() {
        return Err(Problem::field_validation(
            "/effectiveDate",
            "invalid_date",
            "Effective date cannot be in the future.",
        ));
    }
    if body
        .reason
        .as_deref()
        .is_some_and(|reason| reason.chars().count() > 2000)
    {
        return Err(Problem::field_validation(
            "/reason",
            "too_long",
            "Reason cannot exceed 2000 characters.",
        ));
    }
    let (member_id, project_id) =
        crate::repository::pledge::owner(&state.contributions_db, p.organization_id, id)
            .await
            .map_err(db_error)?
            .ok_or_else(Problem::not_found)?;
    full_finance_access(&state.contributions_db, &p, member_id).await?;
    if !project_access(&state.contributions_db, &p, project_id).await? {
        return Err(Problem::forbidden());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /pledges/{id}/{action} {} {}",
        headers
            .get("if-match")
            .and_then(|h| h.to_str().ok())
            .unwrap_or(""),
        json!({"effectiveDate":body.effective_date,"reason":body.reason})
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
    let row = sqlx::query(
        "SELECT status,version,start_date,end_date FROM pledges WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let version: i64 = row.get("version");
    if_match(&headers, id, version)?;
    let status: &str = row.get("status");
    let valid = match action {
        "pause" => status == "active",
        "resume" => status == "paused",
        "cancel" => status == "active",
        _ => false,
    };
    if !valid
        || body.effective_date < row.get::<NaiveDate, _>("start_date")
        || row
            .get::<Option<NaiveDate>, _>("end_date")
            .is_some_and(|end| body.effective_date > end)
    {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "invalid_state",
            "Invalid pledge transition.",
        ));
    }
    sqlx::query("INSERT INTO pledge_events(id,pledge_id,action,effective_date,reason,created_by) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(Uuid::new_v4()).bind(id).bind(action).bind(body.effective_date).bind(&body.reason).bind(p.id).execute(&mut *tx).await.map_err(db_error)?;
    let new_status = match action {
        "pause" => "paused",
        "resume" => "active",
        _ => "cancelled",
    };
    sqlx::query("UPDATE pledges SET status=$2,version=version+1,updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(new_status)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    let calc = recalculate(&mut tx, p.organization_id, member_id, None).await?;
    audit(&mut tx, &p, action, "pledge", id, body.reason.as_deref()).await?;
    let mut resource = pledge_json(&state.contributions_db, &p, id, today()).await?;
    let b = calc.pledge_balances.get(&id).cloned().unwrap_or_default();
    resource["status"] = json!(new_status);
    resource["version"] = json!(version + 1);
    resource["demand"] = json!(b.demand);
    resource["paid"] = json!(b.paid);
    resource["debt"] = json!(b.debt);
    resource["dueToday"] = json!(b.due_today);
    resource["advance"] = json!(b.advance);
    resource["updatedAt"] = json!(Utc::now());
    let envelope = json!({"data":resource});
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
    Ok(item(resource, StatusCode::OK, Some(tag), None))
}

pub(crate) async fn pause_pledge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractJson(body): ContractJson<TransitionBody>,
) -> Result<Response, Problem> {
    transition_pledge(state, headers, id, body, "pause").await
}

pub(crate) async fn resume_pledge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractJson(body): ContractJson<TransitionBody>,
) -> Result<Response, Problem> {
    transition_pledge(state, headers, id, body, "resume").await
}

pub(crate) async fn cancel_pledge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractJson(body): ContractJson<TransitionBody>,
) -> Result<Response, Problem> {
    transition_pledge(state, headers, id, body, "cancel").await
}

pub(crate) async fn add_pledge_version(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractJson(body): ContractJson<VersionBody>,
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
    if body
        .reason
        .as_deref()
        .is_some_and(|reason| reason.chars().count() > 2000)
    {
        return Err(Problem::field_validation(
            "/reason",
            "too_long",
            "Reason cannot exceed 2000 characters.",
        ));
    }
    let (member_id, project_id) =
        crate::repository::pledge::owner(&state.contributions_db, p.organization_id, id)
            .await
            .map_err(db_error)?
            .ok_or_else(Problem::not_found)?;
    full_finance_access(&state.contributions_db, &p, member_id).await?;
    if !project_access(&state.contributions_db, &p, project_id).await? {
        return Err(Problem::forbidden());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /pledges/{id}/versions {} {}",
        headers
            .get("if-match")
            .and_then(|h| h.to_str().ok())
            .unwrap_or(""),
        json!({"id":body.id,"amount":body.amount,"effectiveFrom":body.effective_from,"reason":body.reason})
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
    let row = sqlx::query(
        "SELECT version,start_date,end_date,status FROM pledges WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let version: i64 = row.get("version");
    if_match(&headers, id, version)?;
    if body.effective_from <= row.get::<NaiveDate, _>("start_date")
        || row
            .get::<Option<NaiveDate>, _>("end_date")
            .is_some_and(|end| body.effective_from > end)
        || row.get::<&str, _>("status") == "cancelled"
    {
        return Err(Problem::field_validation(
            "/effectiveFrom",
            "invalid_effective_date",
            "A new amount must take effect after the current pledge start and before its end date. The pledge must not be cancelled.",
        ));
    }
    sqlx::query(
        "INSERT INTO pledge_versions(id,pledge_id,amount,effective_from) VALUES($1,$2,$3,$4)",
    )
    .bind(body.id)
    .bind(id)
    .bind(body.amount)
    .bind(body.effective_from)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    sqlx::query("UPDATE pledges SET version=version+1,updated_at=now() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    let calc = recalculate(&mut tx, p.organization_id, member_id, None).await?;
    audit(
        &mut tx,
        &p,
        "add_version",
        "pledge",
        id,
        body.reason.as_deref(),
    )
    .await?;
    let mut resource = pledge_json(&state.contributions_db, &p, id, today()).await?;
    let b = calc.pledge_balances.get(&id).cloned().unwrap_or_default();
    resource["version"] = json!(version + 1);
    resource["demand"] = json!(b.demand);
    resource["paid"] = json!(b.paid);
    resource["debt"] = json!(b.debt);
    resource["dueToday"] = json!(b.due_today);
    resource["advance"] = json!(b.advance);
    resource["updatedAt"] = json!(Utc::now());
    resource["versions"]
        .as_array_mut()
        .ok_or_else(|| {
            Problem::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid_response_state",
                "Pledge versions could not be updated.",
            )
        })?
        .push(json!({"id":body.id,"amount":body.amount,"effectiveFrom":body.effective_from,"createdAt":Utc::now()}));
    let envelope = json!({"data":resource});
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
    Ok(item(resource, StatusCode::OK, Some(tag), None))
}

pub(crate) async fn pledge_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let (limit, offset) = query.validate(&["effectiveDate", "createdAt"])?;
    let (member_id, project_id) =
        crate::repository::pledge::owner(&state.contributions_db, p.organization_id, id)
            .await
            .map_err(db_error)?
            .ok_or_else(Problem::not_found)?;
    if !member_access(&state.contributions_db, &p, member_id).await?
        || !project_access(&state.contributions_db, &p, project_id).await?
    {
        return Err(Problem::not_found());
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM pledge_events WHERE pledge_id=$1")
        .bind(id)
        .fetch_one(&state.contributions_db)
        .await
        .map_err(db_error)?;
    let rows=sqlx::query("SELECT id,action,effective_date,reason,created_at FROM pledge_events WHERE pledge_id=$1 ORDER BY effective_date,id LIMIT $2 OFFSET $3")
        .bind(id).bind(limit).bind(offset).fetch_all(&state.contributions_db).await.map_err(db_error)?;
    let data=rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"action":r.get::<String,_>("action"),"effectiveDate":r.get::<NaiveDate,_>("effective_date"),"reason":r.get::<Option<String>,_>("reason"),"createdAt":r.get::<DateTime<Utc>,_>("created_at")})).collect();
    Ok(list_response(data, query.meta(total)))
}
