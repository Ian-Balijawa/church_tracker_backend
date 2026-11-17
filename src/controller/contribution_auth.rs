use crate::AppState;
use crate::http::contribution::{ContractJson, Problem, hash, item, no_store};
use crate::model::contribution_auth::*;
use crate::service::contribution_auth::*;
use axum::{
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;
use std::net::SocketAddr;
use uuid::Uuid;

pub(crate) async fn csrf(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    if let Ok(p) = principal(&state, &headers).await {
        return Ok(item(
            json!({"csrfToken": p.csrf_token}),
            StatusCode::OK,
            None,
            None,
        ));
    }
    let token = random_token();
    let mut response = item(json!({"csrfToken": token}), StatusCode::OK, None, None);
    if let Some(value) = make_cookie(
        ANON_COOKIE,
        &token,
        state.config.app_env == "production",
        false,
    ) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

pub(crate) async fn login(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<LoginBody>,
) -> Result<Response, Problem> {
    csrf_check(&state, &headers, None)?;
    if crate::service::redis::is_login_limited(&state.redis, &body.phone, &remote.ip().to_string())
        .await
        .map_err(Problem::internal)?
    {
        return Err(Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many attempts. Try again later.",
        ));
    }
    let row = sqlx::query("SELECT id, organization_id, name, phone, password_hash, role, avatar_url, version, updated_at FROM users WHERE phone=$1 AND status='active'")
        .bind(&body.phone).fetch_optional(&state.contributions_db).await.map_err(Problem::internal)?;
    let verified = if let Some(ref row) = row {
        crate::service::contribution_auth::verify_password(row.get("password_hash"), &body.password)
            .await
    } else {
        false
    };
    if !verified {
        crate::service::redis::record_login_failure(
            &state.redis,
            &body.phone,
            &remote.ip().to_string(),
        )
        .await
        .map_err(Problem::internal)?;
        return Err(Problem::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Invalid credentials.",
        ));
    }
    crate::service::redis::clear_login_failures(
        &state.redis,
        &body.phone,
        &remote.ip().to_string(),
    )
    .await
    .map_err(Problem::internal)?;
    let row = row.ok_or_else(Problem::unauthorized)?;
    let user_id: Uuid = row.get("id");
    let token = random_token();
    let csrf_token = random_token();
    let session_id = Uuid::new_v4();
    sqlx::query("INSERT INTO sessions(id,user_id,token_hash,csrf_token,expires_at) VALUES($1,$2,$3,$4,now()+interval '14 days')")
        .bind(session_id).bind(user_id).bind(hash(&token)).bind(&csrf_token).execute(&state.contributions_db).await.map_err(Problem::internal)?;
    let church_ids =
        crate::repository::contribution_auth::church_ids(&state.contributions_db, user_id)
            .await
            .map_err(Problem::internal)?;
    let user = json!({"id":user_id,"name":row.get::<String,_>("name"),"phone":row.get::<String,_>("phone"),"avatarUrl":row.get::<Option<String>,_>("avatar_url"),"role":row.get::<String,_>("role"),"churchIds":church_ids,"updatedAt":row.get::<DateTime<Utc>,_>("updated_at"),"version":row.get::<i64,_>("version")});
    let mut response = item(
        json!({"user":user,"session":{"id":session_id,"createdAt":Utc::now(),"current":true},"csrfToken":csrf_token}),
        StatusCode::OK,
        None,
        None,
    );
    if let Some(value) = make_cookie(
        SESSION_COOKIE,
        &token,
        state.config.app_env == "production",
        false,
    ) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    if let Some(value) = make_cookie(ANON_COOKIE, "", state.config.app_env == "production", true) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

pub(crate) async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let row =
        sqlx::query("SELECT name,phone,role,avatar_url,updated_at,version FROM users WHERE id=$1")
            .bind(p.id)
            .fetch_one(&state.contributions_db)
            .await
            .map_err(Problem::internal)?;
    let church_ids =
        crate::repository::contribution_auth::church_ids(&state.contributions_db, p.id)
            .await
            .map_err(Problem::internal)?;
    let user = json!({"id":p.id,"name":row.get::<String,_>("name"),"phone":row.get::<String,_>("phone"),"avatarUrl":row.get::<Option<String>,_>("avatar_url"),"role":row.get::<String,_>("role"),"churchIds":church_ids,"updatedAt":row.get::<DateTime<Utc>,_>("updated_at"),"version":row.get::<i64,_>("version")});
    Ok(item(
        json!({"user":user,"session":{"id":p.session_id,"current":true},"csrfToken":p.csrf_token}),
        StatusCode::OK,
        None,
        None,
    ))
}

pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await.ok();
    csrf_check(&state, &headers, p.as_ref())?;
    if let Some(p) = p {
        sqlx::query("UPDATE sessions SET revoked_at=now() WHERE id=$1")
            .bind(p.session_id)
            .execute(&state.contributions_db)
            .await
            .map_err(Problem::internal)?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Some(value) = make_cookie(
        SESSION_COOKIE,
        "",
        state.config.app_env == "production",
        true,
    ) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(no_store(response))
}

pub(crate) async fn sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let p = principal(&state, &headers).await?;
    let rows=sqlx::query("SELECT id,created_at,last_seen_at,device_label FROM sessions WHERE user_id=$1 AND revoked_at IS NULL AND expires_at>now() ORDER BY created_at DESC,id")
        .bind(p.id).fetch_all(&state.contributions_db).await.map_err(Problem::internal)?;
    let data=rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"createdAt":r.get::<DateTime<Utc>,_>("created_at"),"lastSeenAt":r.get::<DateTime<Utc>,_>("last_seen_at"),"deviceLabel":r.get::<Option<String>,_>("device_label"),"current":r.get::<Uuid,_>("id")==p.session_id})).collect::<Vec<_>>();
    Ok(item(data, StatusCode::OK, None, None))
}
pub(crate) async fn revoke_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(p.id)
        .execute(&state.contributions_db)
        .await
        .map_err(Problem::internal)?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    if id == p.session_id {
        if let Some(value) = make_cookie(
            SESSION_COOKIE,
            "",
            state.config.app_env == "production",
            true,
        ) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    Ok(no_store(response))
}

pub(crate) async fn unlock(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<UnlockBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    if !matches!(body.method.as_str(), "password" | "pin") {
        return Err(Problem::validation());
    }
    if crate::service::redis::request_limited(
        &state.redis,
        "unlock",
        &p.id.to_string(),
        &remote.ip().to_string(),
        5,
    )
    .await
    .map_err(Problem::internal)?
    {
        return Err(Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many attempts. Try again later.",
        ));
    }
    let row = sqlx::query("SELECT password_hash,pin_hash FROM users WHERE id=$1")
        .bind(p.id)
        .fetch_one(&state.contributions_db)
        .await
        .map_err(Problem::internal)?;
    let secret = if body.method == "pin" {
        row.get::<Option<String>, _>("pin_hash")
    } else {
        Some(row.get::<String, _>("password_hash"))
    };
    let valid = if let Some(secret) = secret {
        crate::service::contribution_auth::verify_password(&secret, &body.credential).await
    } else {
        false
    };
    if !valid {
        return Err(Problem::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Invalid credentials.",
        ));
    }
    Ok(no_store(StatusCode::NO_CONTENT.into_response()))
}

pub(crate) async fn set_pin(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<SetPinBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    if !(4..=12).contains(&body.new_pin.len()) || !body.new_pin.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Problem::validation());
    }
    verify_current_password(&state, &p, &body.current_password).await?;
    let hashed = crate::service::contribution_auth::hash_password(&body.new_pin)
        .await
        .map_err(Problem::internal)?;
    sqlx::query("UPDATE users SET pin_hash=$2,updated_at=now(),version=version+1 WHERE id=$1")
        .bind(p.id)
        .bind(hashed)
        .execute(&state.contributions_db)
        .await
        .map_err(Problem::internal)?;
    Ok(no_store(StatusCode::NO_CONTENT.into_response()))
}

pub(crate) async fn delete_pin(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<PasswordBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    if body.new_password.is_some() {
        return Err(Problem::validation());
    }
    verify_current_password(&state, &p, &body.current_password).await?;
    sqlx::query("UPDATE users SET pin_hash=NULL,updated_at=now(),version=version+1 WHERE id=$1")
        .bind(p.id)
        .execute(&state.contributions_db)
        .await
        .map_err(Problem::internal)?;
    Ok(no_store(StatusCode::NO_CONTENT.into_response()))
}
pub(crate) async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<PasswordBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    let new = body.new_password.ok_or_else(Problem::validation)?;
    if new.len() < 8 || new.len() > 1024 {
        return Err(Problem::validation());
    }
    verify_current_password(&state, &p, &body.current_password).await?;
    let hashed = crate::service::contribution_auth::hash_password(&new)
        .await
        .map_err(Problem::internal)?;
    let token = random_token();
    let csrf = random_token();
    let mut tx = state
        .contributions_db
        .begin()
        .await
        .map_err(Problem::internal)?;
    sqlx::query("UPDATE users SET password_hash=$2,updated_at=now(),version=version+1 WHERE id=$1")
        .bind(p.id)
        .bind(hashed)
        .execute(&mut *tx)
        .await
        .map_err(Problem::internal)?;
    sqlx::query(
        "UPDATE sessions SET revoked_at=now() WHERE user_id=$1 AND id<>$2 AND revoked_at IS NULL",
    )
    .bind(p.id)
    .bind(p.session_id)
    .execute(&mut *tx)
    .await
    .map_err(Problem::internal)?;
    sqlx::query("UPDATE sessions SET token_hash=$2,csrf_token=$3 WHERE id=$1")
        .bind(p.session_id)
        .bind(hash(&token))
        .bind(csrf)
        .execute(&mut *tx)
        .await
        .map_err(Problem::internal)?;
    tx.commit().await.map_err(Problem::internal)?;
    let mut response = no_store(StatusCode::NO_CONTENT.into_response());
    if let Some(value) = make_cookie(
        SESSION_COOKIE,
        &token,
        state.config.app_env == "production",
        false,
    ) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

pub(crate) async fn create_admin_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    ContractJson(body): ContractJson<AdminUserBody>,
) -> Result<Response, Problem> {
    let p = write_principal(&state, &headers).await?;
    if p.role != "organization_admin" {
        return Err(Problem::forbidden());
    }
    if body.name.trim().is_empty()
        || body.name.chars().count() > 200
        || body.password.len() < 8
        || body.phone.len() < 8
        || !body.phone.starts_with('+')
        || !body.phone[1..].bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Problem::validation());
    }
    let hashed = crate::service::contribution_auth::hash_password(&body.password)
        .await
        .map_err(Problem::internal)?;
    let id = Uuid::new_v4();
    let mut tx = state
        .contributions_db
        .begin()
        .await
        .map_err(Problem::internal)?;
    sqlx::query("INSERT INTO users(id,organization_id,name,phone,password_hash,role) VALUES($1,$2,$3,$4,$5,'organization_admin')")
        .bind(id).bind(p.organization_id).bind(body.name.trim()).bind(&body.phone).bind(hashed).execute(&mut *tx).await.map_err(|error|if error.as_database_error().is_some_and(|e|e.is_unique_violation()){Problem::conflict()}else{Problem::internal(error)})?;
    sqlx::query("INSERT INTO audit(id,organization_id,actor_id,action,entity_type,entity_id,request_id) VALUES($1,$2,$3,'create','user',$4,$5)")
        .bind(Uuid::new_v4()).bind(p.organization_id).bind(p.id).bind(id).bind(Uuid::new_v4()).execute(&mut *tx).await.map_err(Problem::internal)?;
    tx.commit().await.map_err(Problem::internal)?;
    Ok(item(
        json!({"id":id,"name":body.name.trim(),"phone":body.phone,"role":"organization_admin","churchIds":[],"status":"active","createdAt":Utc::now(),"updatedAt":Utc::now(),"version":1}),
        StatusCode::CREATED,
        Some(crate::http::contribution::etag(id, 1)),
        Some(format!("/api/v1/users/{id}")),
    ))
}
