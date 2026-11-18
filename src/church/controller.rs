use crate::AppState;
use crate::http::contribution::{
    ContractJson, ContractQuery, Problem, etag, hash, if_match, item, list_response, req_key,
};
use crate::model::church::*;
use crate::model::contribution::*;
use crate::repository::contribution::*;
use crate::service::church::*;
use crate::service::contribution::*;
use crate::service::contribution_auth::{
    church_access, principal, require_manager, write_principal,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn create_church(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<ChurchCreate>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_manager(&p)?;
    if !valid_name(&body.name) {
        return Err(Problem::field_validation(
            "/name",
            "invalid_name",
            "Name must contain between 1 and 200 characters.",
        ));
    }
    if body
        .phone
        .as_deref()
        .is_some_and(|phone| !valid_phone(phone))
    {
        return Err(Problem::field_validation(
            "/phone",
            "invalid_phone",
            "Enter a valid phone number or omit phone.",
        ));
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /churches {}",
        json!({"id":body.id,"name":body.name,"phone":body.phone})
    ));
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    sqlx::query("INSERT INTO churches(id,organization_id,name,phone) VALUES($1,$2,$3,$4)")
        .bind(body.id)
        .bind(p.organization_id)
        .bind(body.name.trim())
        .bind(&body.phone)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    audit(&mut tx, &p, "create", "church", body.id, None).await?;
    let resource = json!({"id":body.id,"name":body.name.trim(),"phone":body.phone,"status":"active","memberCount":0,"projectCount":0,"version":1,"createdAt":Utc::now(),"updatedAt":Utc::now()});
    let envelope = json!({"data":resource});
    let tag = etag(body.id, 1);
    let location = format!("/api/v1/churches/{}", body.id);
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

pub(crate) async fn get_church(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let value = church_json(&state.contributions_db, &p, id).await?;
    Ok(item(
        &value,
        StatusCode::OK,
        Some(etag(id, value["version"].as_i64().unwrap_or(1))),
        None,
    ))
}

pub(crate) async fn list_churches(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let (limit, offset) = query.validate(&["name", "memberCount", "createdAt"])?;
    let rows=sqlx::query("SELECT id FROM churches c WHERE c.organization_id=$1 AND c.deleted_at IS NULL AND ($2='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$3 AND uc.church_id=c.id)) AND ($4='' OR lower(c.name) LIKE lower('%'||$4||'%')) ORDER BY c.name,c.id LIMIT $5 OFFSET $6")
        .bind(p.organization_id).bind(&p.role).bind(p.id).bind(query.q.as_deref().unwrap_or("").trim()).bind(limit).bind(offset).fetch_all(&state.contributions_db).await.map_err(db_error)?;
    let total: i64=sqlx::query_scalar("SELECT count(*) FROM churches c WHERE c.organization_id=$1 AND c.deleted_at IS NULL AND ($2='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$3 AND uc.church_id=c.id)) AND ($4='' OR lower(c.name) LIKE lower('%'||$4||'%'))")
        .bind(p.organization_id).bind(&p.role).bind(p.id).bind(query.q.as_deref().unwrap_or("").trim()).fetch_one(&state.contributions_db).await.map_err(db_error)?;
    let mut data = Vec::with_capacity(rows.len());
    for row in rows {
        data.push(church_json(&state.contributions_db, &p, row.get("id")).await?);
    }
    Ok(list_response(data, query.meta(total)))
}

pub(crate) async fn patch_church(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractJson(body): ContractJson<ChurchPatch>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_manager(&p)?;
    if !church_access(&state.contributions_db, &p, id).await? {
        return Err(Problem::not_found());
    }
    if body.name.as_deref().is_some_and(|name| !valid_name(name)) {
        return Err(Problem::field_validation(
            "/name",
            "invalid_name",
            "Name must contain between 1 and 200 characters.",
        ));
    }
    if body
        .phone
        .as_ref()
        .and_then(|phone| phone.as_deref())
        .is_some_and(|phone| !valid_phone(phone))
    {
        return Err(Problem::field_validation(
            "/phone",
            "invalid_phone",
            "Enter a valid phone number or set phone to null.",
        ));
    }
    let key = req_key(&headers)?;
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    let fingerprint = hash(&format!(
        "PATCH /churches/{id} {} {}",
        headers
            .get("if-match")
            .and_then(|h| h.to_str().ok())
            .unwrap_or(""),
        json!({"name":body.name,"phone":body.phone})
    ));
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    let version = crate::repository::church::version_for_update(&mut tx, p.organization_id, id)
        .await
        .map_err(db_error)?
        .ok_or_else(Problem::not_found)?;
    if_match(&headers, id, version)?;
    sqlx::query("UPDATE churches SET name=COALESCE($2,name),phone=CASE WHEN $3 THEN $4 ELSE phone END,version=version+1,updated_at=now() WHERE id=$1")
        .bind(id).bind(body.name.as_deref().map(str::trim)).bind(body.phone.is_some()).bind(body.phone.flatten()).execute(&mut *tx).await.map_err(db_error)?;
    audit(&mut tx, &p, "update", "church", id, None).await?;
    let tag = etag(id, version + 1);
    let row = sqlx::query(
        "SELECT name,phone,status,version,created_at,updated_at FROM churches WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let member_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM members WHERE home_church_id=$1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let project_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM projects WHERE owner_church_id=$1 AND deleted_at IS NULL",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_error)?;
    let resource = json!({"id":id,"name":row.get::<String,_>("name"),"phone":row.get::<Option<String>,_>("phone"),"status":row.get::<String,_>("status"),"memberCount":member_count,"projectCount":project_count,"version":row.get::<i64,_>("version"),"createdAt":row.get::<DateTime<Utc>,_>("created_at"),"updatedAt":row.get::<DateTime<Utc>,_>("updated_at")});
    let envelope = json!({"data":resource});
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
