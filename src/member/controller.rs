use crate::AppState;
use crate::http::contribution::{
    ContractJson, ContractQuery, Problem, etag, hash, item, list_response, req_key,
};
use crate::model::contribution::*;
use crate::model::member::*;
use crate::repository::contribution::*;
use crate::service::contribution::*;
use crate::service::contribution_auth::{
    church_access, principal, require_manager, write_principal,
};
use crate::service::member::*;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn create_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<MemberCreate>,
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
    if !valid_phone(&body.phone) {
        return Err(Problem::field_validation(
            "/phone",
            "invalid_phone",
            "Enter a valid phone number.",
        ));
    }
    if !church_access(&state.contributions_db, &p, body.home_church_id).await? {
        return Err(Problem::forbidden());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /members {}",
        json!({"id":body.id,"name":body.name,"phone":body.phone,"homeChurchId":body.home_church_id})
    ));
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    sqlx::query(
        "INSERT INTO members(id,organization_id,name,phone,home_church_id) VALUES($1,$2,$3,$4,$5)",
    )
    .bind(body.id)
    .bind(p.organization_id)
    .bind(body.name.trim())
    .bind(&body.phone)
    .bind(body.home_church_id)
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    sqlx::query("INSERT INTO member_churches(id,organization_id,member_id,church_id,created_by) VALUES($1,$2,$3,$4,$5)").bind(Uuid::new_v4()).bind(p.organization_id).bind(body.id).bind(body.home_church_id).bind(p.id).execute(&mut *tx).await.map_err(db_error)?;
    audit(&mut tx, &p, "create", "member", body.id, None).await?;
    let resource = json!({"id":body.id,"name":body.name.trim(),"phone":body.phone,"homeChurchId":body.home_church_id,"status":"active","churchIds":[body.home_church_id],"projectIds":[],"totalGiven":0,"debt":0,"dueToday":0,"advance":0,"unallocatedCredit":0,"version":1,"createdAt":Utc::now(),"updatedAt":Utc::now()});
    let envelope = json!({"data":resource});
    let tag = etag(body.id, 1);
    let location = format!("/api/v1/members/{}", body.id);
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

pub(crate) async fn get_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    query.validate(&["name"])?;
    let value = member_json(
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

pub(crate) async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let (limit, offset) = query.validate(&[
        "name",
        "phone",
        "totalGiven",
        "debt",
        "advance",
        "createdAt",
    ])?;
    let ids=sqlx::query("SELECT m.id FROM members m WHERE m.organization_id=$1 AND m.deleted_at IS NULL AND ($2='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$3 AND (uc.church_id=m.home_church_id OR EXISTS(SELECT 1 FROM member_churches mc WHERE mc.member_id=m.id AND mc.church_id=uc.church_id AND mc.detached_at IS NULL)))) AND ($4='' OR lower(m.name) LIKE lower('%'||$4||'%')) ORDER BY m.name,m.id LIMIT $5 OFFSET $6")
        .bind(p.organization_id).bind(&p.role).bind(p.id).bind(query.q.as_deref().unwrap_or("").trim()).bind(limit).bind(offset).fetch_all(&state.contributions_db).await.map_err(db_error)?;
    let total:i64=sqlx::query_scalar("SELECT count(*) FROM members m WHERE m.organization_id=$1 AND m.deleted_at IS NULL AND ($2='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$3 AND (uc.church_id=m.home_church_id OR EXISTS(SELECT 1 FROM member_churches mc WHERE mc.member_id=m.id AND mc.church_id=uc.church_id AND mc.detached_at IS NULL)))) AND ($4='' OR lower(m.name) LIKE lower('%'||$4||'%'))")
        .bind(p.organization_id).bind(&p.role).bind(p.id).bind(query.q.as_deref().unwrap_or("").trim()).fetch_one(&state.contributions_db).await.map_err(db_error)?;
    let mut data = Vec::with_capacity(ids.len());
    for row in ids {
        data.push(
            member_json(
                &state.contributions_db,
                &p,
                row.get("id"),
                query.as_of.unwrap_or_else(today),
            )
            .await?,
        );
    }
    Ok(list_response(data, query.meta(total)))
}
