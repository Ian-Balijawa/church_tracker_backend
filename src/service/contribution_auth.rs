use crate::AppState;
use crate::http::contribution::{Problem, hash};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use sqlx::{PgPool, Row};
use uuid::Uuid;
pub(crate) const SESSION_COOKIE: &str = "mc_session";
pub(crate) const ANON_COOKIE: &str = "mc_csrf_anon";

#[derive(Clone)]
pub(crate) struct Principal {
    pub(crate) id: Uuid,
    pub(crate) organization_id: Uuid,
    pub(crate) role: String,
    pub(crate) session_id: Uuid,
    pub(crate) csrf_token: String,
}
pub(crate) async fn church_access(
    db: &PgPool,
    p: &Principal,
    church_id: Uuid,
) -> Result<bool, Problem> {
    let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM churches c WHERE c.id=$1 AND c.organization_id=$2 AND c.deleted_at IS NULL AND ($3='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$4 AND uc.church_id=c.id)))")
        .bind(church_id).bind(p.organization_id).bind(&p.role).bind(p.id).fetch_one(db).await.map_err(Problem::internal)?;
    Ok(found)
}

pub(crate) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name).then_some(value)
        })
}

pub(crate) async fn create_admin(
    db: &PgPool,
    organization: &str,
    name: &str,
    phone: &str,
    password: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !organization.trim().is_empty()
            && !name.trim().is_empty()
            && phone.starts_with('+')
            && phone[1..].bytes().all(|byte| byte.is_ascii_digit()),
        "invalid admin details"
    );
    let password_hash = crate::service::contribution_auth::hash_password(password).await?;
    let mut tx = db.begin().await?;
    let organization_id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,$2)")
        .bind(organization_id)
        .bind(organization.trim())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO users(id,organization_id,name,phone,password_hash,role) VALUES($1,$2,$3,$4,$5,'organization_admin')")
        .bind(Uuid::new_v4()).bind(organization_id).bind(name.trim()).bind(phone).bind(password_hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) fn csrf_check(
    state: &AppState,
    headers: &HeaderMap,
    principal: Option<&Principal>,
) -> Result<(), Problem> {
    origin(state, headers)?;
    let supplied = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            Problem::new(
                StatusCode::FORBIDDEN,
                "csrf_invalid",
                "CSRF token is required.",
            )
        })?;
    let expected = principal
        .map(|p| p.csrf_token.as_str())
        .or_else(|| cookie(headers, ANON_COOKIE))
        .ok_or_else(|| {
            Problem::new(
                StatusCode::FORBIDDEN,
                "csrf_invalid",
                "CSRF token is required.",
            )
        })?;
    if hash(supplied) != hash(expected) {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "csrf_invalid",
            "CSRF token is invalid.",
        ));
    }
    Ok(())
}

pub(crate) fn make_cookie(
    name: &str,
    token: &str,
    production: bool,
    clear: bool,
) -> Option<HeaderValue> {
    HeaderValue::from_str(&format!(
        "{name}={token}; Path=/api/v1; HttpOnly; SameSite=Lax{}{}",
        if production { "; Secure" } else { "" },
        if clear { "; Max-Age=0" } else { "" }
    ))
    .ok()
}

pub(crate) async fn member_access(
    db: &PgPool,
    p: &Principal,
    member_id: Uuid,
) -> Result<bool, Problem> {
    let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM members m WHERE m.id=$1 AND m.organization_id=$2 AND m.deleted_at IS NULL AND ($3='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$4 AND (uc.church_id=m.home_church_id OR EXISTS(SELECT 1 FROM member_churches mc WHERE mc.member_id=m.id AND mc.church_id=uc.church_id AND mc.detached_at IS NULL)))))")
        .bind(member_id).bind(p.organization_id).bind(&p.role).bind(p.id).fetch_one(db).await.map_err(Problem::internal)?;
    Ok(found)
}

pub(crate) fn origin(state: &AppState, headers: &HeaderMap) -> Result<(), Problem> {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            Problem::new(StatusCode::FORBIDDEN, "csrf_invalid", "Origin is required.")
        })?;
    if !state
        .config
        .cors_origins
        .iter()
        .any(|allowed| allowed == origin)
    {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "csrf_invalid",
            "Origin is not allowed.",
        ));
    }
    Ok(())
}

pub(crate) async fn principal(state: &AppState, headers: &HeaderMap) -> Result<Principal, Problem> {
    let token = cookie(headers, SESSION_COOKIE).ok_or_else(Problem::unauthorized)?;
    let row = sqlx::query("SELECT s.id AS session_id, s.csrf_token, u.id, u.organization_id, u.role FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now() AND u.status='active'")
        .bind(hash(token)).fetch_optional(&state.contributions_db).await.map_err(Problem::internal)?
        .ok_or_else(Problem::unauthorized)?;
    Ok(Principal {
        id: row.get("id"),
        organization_id: row.get("organization_id"),
        role: row.get("role"),
        session_id: row.get("session_id"),
        csrf_token: row.get("csrf_token"),
    })
}

pub(crate) async fn project_access(
    db: &PgPool,
    p: &Principal,
    project_id: Uuid,
) -> Result<bool, Problem> {
    let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects x WHERE x.id=$1 AND x.organization_id=$2 AND x.deleted_at IS NULL AND ($3='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$4 AND (uc.church_id=x.owner_church_id OR EXISTS(SELECT 1 FROM project_churches pc WHERE pc.project_id=x.id AND pc.church_id=uc.church_id AND pc.detached_at IS NULL)))))")
        .bind(project_id).bind(p.organization_id).bind(&p.role).bind(p.id).fetch_one(db).await.map_err(Problem::internal)?;
    Ok(found)
}

pub(crate) fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub(crate) fn require_finance(p: &Principal) -> Result<(), Problem> {
    if matches!(
        p.role.as_str(),
        "organization_admin" | "church_admin" | "treasurer"
    ) {
        Ok(())
    } else {
        Err(Problem::forbidden())
    }
}

pub(crate) fn require_manager(p: &Principal) -> Result<(), Problem> {
    if matches!(p.role.as_str(), "organization_admin" | "church_admin") {
        Ok(())
    } else {
        Err(Problem::forbidden())
    }
}

pub(crate) async fn verify_current_password(
    state: &AppState,
    p: &Principal,
    password: &str,
) -> Result<(), Problem> {
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=$1")
        .bind(p.id)
        .fetch_one(&state.contributions_db)
        .await
        .map_err(Problem::internal)?;
    if !crate::service::contribution_auth::verify_password(&hash, password).await {
        return Err(Problem::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Invalid credentials.",
        ));
    }
    Ok(())
}

pub(crate) async fn write_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Principal, Problem> {
    let p = principal(state, headers).await?;
    csrf_check(state, headers, Some(&p))?;
    Ok(p)
}

pub(crate) async fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::{Argon2, PasswordHasher};
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    })
    .await?
}

pub(crate) async fn verify_password(hash: &str, password: &str) -> bool {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    let hash = hash.to_owned();
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).ok().is_some_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}
