use crate::AppState;
use crate::http::contribution::{
    ContractJson, ContractQuery, Problem, etag, hash, if_match, item, list_response, no_store,
    req_key,
};
use crate::model::contribution::*;
use crate::model::project::*;
use crate::repository::contribution::*;
use crate::service::contribution::*;
use crate::service::contribution_auth::{
    church_access, member_access, principal, project_access, require_manager, write_principal,
};
use crate::service::project::*;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn create_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<ProjectCreate>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_manager(&p)?;
    if !valid_name(&body.name)
        || body.description.as_deref().is_some_and(|v| v.len() > 2000)
        || !valid_money(body.target, false)
        || body.end_date.is_some_and(|end| end < body.start_date)
    {
        return Err(Problem::validation());
    }
    if !church_access(&state.contributions_db, &p, body.owner_church_id).await? {
        return Err(Problem::forbidden());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "POST /projects {}",
        json!({"id":body.id,"name":body.name,"description":body.description,"ownerChurchId":body.owner_church_id,"target":body.target,"startDate":body.start_date,"endDate":body.end_date})
    ));
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    sqlx::query("INSERT INTO projects(id,organization_id,name,description,owner_church_id,target,start_date,end_date) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(body.id).bind(p.organization_id).bind(body.name.trim()).bind(&body.description).bind(body.owner_church_id).bind(body.target).bind(body.start_date).bind(body.end_date).execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("INSERT INTO project_churches(id,organization_id,project_id,church_id,created_by) VALUES($1,$2,$3,$4,$5)").bind(Uuid::new_v4()).bind(p.organization_id).bind(body.id).bind(body.owner_church_id).bind(p.id).execute(&mut *tx).await.map_err(db_error)?;
    audit(&mut tx, &p, "create", "project", body.id, None).await?;
    let tag = etag(body.id, 1);
    let location = format!("/api/v1/projects/{}", body.id);
    let resource = json!({"id":body.id,"name":body.name.trim(),"description":body.description,"ownerChurchId":body.owner_church_id,"target":body.target,"startDate":body.start_date,"endDate":body.end_date,"status":"active","churchIds":[body.owner_church_id],"memberCount":0,"collected":0,"debt":0,"dueToday":0,"advance":0,"targetProgressBasisPoints":if body.target==0{None}else{Some(0i64)},"version":1,"createdAt":Utc::now(),"updatedAt":Utc::now()});
    let envelope = json!({"data":resource});
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

pub(crate) async fn get_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    query.validate(&["name"])?;
    let value = project_json(
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

pub(crate) async fn list_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractQuery(query): ContractQuery<ListQuery>,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let (limit, offset) = query.validate(&["name", "target", "collected", "createdAt"])?;
    let ids=sqlx::query("SELECT x.id FROM projects x WHERE x.organization_id=$1 AND x.deleted_at IS NULL AND ($2='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$3 AND (uc.church_id=x.owner_church_id OR EXISTS(SELECT 1 FROM project_churches pc WHERE pc.project_id=x.id AND pc.church_id=uc.church_id AND pc.detached_at IS NULL)))) AND ($4='' OR lower(x.name) LIKE lower('%'||$4||'%')) ORDER BY x.name,x.id LIMIT $5 OFFSET $6")
        .bind(p.organization_id).bind(&p.role).bind(p.id).bind(query.q.as_deref().unwrap_or("").trim()).bind(limit).bind(offset).fetch_all(&state.contributions_db).await.map_err(db_error)?;
    let total:i64=sqlx::query_scalar("SELECT count(*) FROM projects x WHERE x.organization_id=$1 AND x.deleted_at IS NULL AND ($2='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$3 AND (uc.church_id=x.owner_church_id OR EXISTS(SELECT 1 FROM project_churches pc WHERE pc.project_id=x.id AND pc.church_id=uc.church_id AND pc.detached_at IS NULL)))) AND ($4='' OR lower(x.name) LIKE lower('%'||$4||'%'))")
        .bind(p.organization_id).bind(&p.role).bind(p.id).bind(query.q.as_deref().unwrap_or("").trim()).fetch_one(&state.contributions_db).await.map_err(db_error)?;
    let mut data = Vec::with_capacity(ids.len());
    for row in ids {
        data.push(
            project_json(
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

pub(crate) async fn attach_project_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, member_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    require_manager(&p)?;
    if !project_access(&state.contributions_db, &p, id).await?
        || !member_access(&state.contributions_db, &p, member_id).await?
    {
        return Err(Problem::forbidden());
    }
    let key = req_key(&headers)?;
    let fingerprint = hash(&format!(
        "PUT /projects/{id}/members/{member_id} {}",
        headers
            .get("if-match")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
    ));
    let mut tx = state.contributions_db.begin().await.map_err(db_error)?;
    if let Some(response) = replay_check(&mut tx, &p, key, &fingerprint).await? {
        return Ok(response);
    }
    let version = crate::repository::project::version_for_update(&mut tx, p.organization_id, id)
        .await
        .map_err(db_error)?
        .ok_or_else(Problem::not_found)?;
    if_match(&headers, id, version)?;
    sqlx::query("INSERT INTO project_members(id,organization_id,project_id,member_id,created_by) VALUES($1,$2,$3,$4,$5)").bind(Uuid::new_v4()).bind(p.organization_id).bind(id).bind(member_id).bind(p.id).execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("UPDATE projects SET version=version+1,updated_at=now() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
    audit(&mut tx, &p, "attach_member", "project", id, None).await?;
    let tag = etag(id, version + 1);
    let envelope = json!(null);
    save_replay(
        &mut tx,
        &p,
        key,
        StatusCode::NO_CONTENT,
        &envelope,
        None,
        Some(&tag),
    )
    .await?;
    tx.commit().await.map_err(db_error)?;
    let mut response = no_store(StatusCode::NO_CONTENT.into_response());
    response.headers_mut().insert(
        axum::http::header::ETAG,
        tag.parse().map_err(Problem::internal)?,
    );
    Ok(response)
}
