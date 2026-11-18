use crate::AppState;
use crate::http::contribution::{Problem, hash};
use crate::model::contribution_auth::RegisterBody;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use sqlx::{PgPool, Row};
use uuid::Uuid;

const OTP_TTL_SECONDS: u64 = 300;
const OTP_RATE_TTL_SECONDS: u64 = 60;
const OTP_RATE_LIMIT: i64 = 5;

/// Normalize Ugandan phone input to the canonical `2567xxxxxxxx` form.
pub(crate) fn format_phone(phone: &str) -> Option<String> {
    let mut digits = phone
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect::<String>();
    if digits.starts_with("256") {
        while digits.starts_with("2560") {
            digits.remove(3);
        }
    } else if let Some(local) = digits.strip_prefix('0') {
        digits = format!("256{local}");
    } else if digits.starts_with('7') {
        digits.insert_str(0, "256");
    }
    (digits.len() == 12 && digits.starts_with("2567")).then_some(digits)
}

fn otp_key(kind: &str, destination: &str, scope: &str) -> String {
    format!(
        "auth:otp:{kind}:{}",
        hash(&format!("{scope}:{destination}"))
    )
}

async fn generate_otp(
    state: &AppState,
    kind: &str,
    destination: &str,
    scope: &str,
) -> Result<String, Problem> {
    use redis::AsyncCommands;
    let rate_key = otp_key(&format!("{kind}:rate"), destination, scope);
    let code_key = otp_key(kind, destination, scope);
    let mut redis = state.redis.clone();
    let count: i64 = redis::Script::new(
        "local count=redis.call('INCR',KEYS[1]); if count == 1 then redis.call('EXPIRE',KEYS[1],ARGV[1]); end; return count",
    )
    .key(&rate_key)
    .arg(OTP_RATE_TTL_SECONDS)
    .invoke_async(&mut redis)
    .await
    .map_err(Problem::internal)?;
    if count > OTP_RATE_LIMIT {
        return Err(Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many verification code requests. Try again later.",
        ));
    }
    let number = Uuid::new_v4().as_u128() % 100_000;
    let code = format!("{number:05}");
    let _: () = redis
        .set_ex(&code_key, hash(&code), OTP_TTL_SECONDS)
        .await
        .map_err(Problem::internal)?;
    let _: usize = redis
        .del(otp_key(&format!("{kind}:attempts"), destination, scope))
        .await
        .map_err(Problem::internal)?;
    Ok(code)
}

async fn consume_otp(
    state: &AppState,
    kind: &str,
    destination: &str,
    scope: &str,
    code: &str,
) -> Result<(), Problem> {
    use redis::AsyncCommands;
    let key = otp_key(kind, destination, scope);
    let mut redis = state.redis.clone();
    let valid: i64 = redis::Script::new(
        "local stored=redis.call('GET',KEYS[1]); if not stored then return 0 end; if stored ~= ARGV[1] then return 2 end; redis.call('DEL',KEYS[1]); return 1",
    )
    .key(&key)
    .arg(hash(code))
    .invoke_async(&mut redis)
    .await
    .map_err(Problem::internal)?;
    if valid == 0 {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "otp_expired",
            "Verification code expired or was not found.",
        ));
    }
    if valid == 2 {
        let attempts_key = otp_key(&format!("{kind}:attempts"), destination, scope);
        let attempts: i64 = redis::cmd("INCR")
            .arg(&attempts_key)
            .query_async(&mut redis)
            .await
            .map_err(Problem::internal)?;
        if attempts == 1 {
            let _: bool = redis::cmd("EXPIRE")
                .arg(&attempts_key)
                .arg(OTP_TTL_SECONDS)
                .query_async(&mut redis)
                .await
                .map_err(Problem::internal)?;
        }
        if attempts >= 5 {
            let _: usize = redis.del(&key).await.map_err(Problem::internal)?;
        }
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "otp_invalid",
            "Verification code is invalid.",
        ));
    }
    let _: usize = redis
        .del(otp_key(&format!("{kind}:attempts"), destination, scope))
        .await
        .map_err(Problem::internal)?;
    Ok(())
}

async fn send_sms_otp(state: &AppState, phone: &str, code: &str) -> Result<(), Problem> {
    let (Some(username), Some(password), Some(sender_id)) = (
        state.config.sms_username.as_deref(),
        state.config.sms_password.as_deref(),
        state.config.sms_sender_id.as_deref(),
    ) else {
        return Err(Problem::internal("SMS provider is not configured"));
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(Problem::internal)?;
    let response = client.post("https://www.egosms.co/api/v1/json/").json(&serde_json::json!({
        "method": "SendSms",
        "userdata": {"username": username, "password": password},
        "msgdata": [{"number": phone, "message": format!("Your verification code is: {code}"), "senderid": sender_id, "priority": "1"}]
    })).send().await.map_err(Problem::internal)?;
    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(Problem::internal)?;
    if !status.is_success() || body.get("Status").and_then(serde_json::Value::as_str) != Some("OK")
    {
        return Err(Problem::internal("SMS provider rejected the message"));
    }
    Ok(())
}

async fn send_email_otp(state: &AppState, email: &str, code: &str) -> Result<(), Problem> {
    use lettre::{
        AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
        transport::smtp::authentication::Credentials,
    };
    let (server, user, password) = (
        state.config.smtp_server.as_str(),
        state.config.smtp_user.as_str(),
        state.config.smtp_pass.as_str(),
    );
    if user.is_empty() || password.is_empty() {
        return Err(Problem::internal("email provider is not configured"));
    }
    let message = Message::builder()
        .from(user.parse().map_err(Problem::internal)?)
        .to(email.parse().map_err(Problem::internal)?)
        .subject("Your verification code")
        .body(format!(
            "Your verification code is {code}. It expires in 5 minutes."
        ))
        .map_err(Problem::internal)?;
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::relay(server)
        .map_err(Problem::internal)?
        .port(state.config.smtp_port)
        .credentials(Credentials::new(user.to_owned(), password.to_owned()))
        .build();
    mailer.send(message).await.map_err(Problem::internal)?;
    Ok(())
}

pub(crate) async fn register(
    state: &AppState,
    body: RegisterBody,
) -> Result<serde_json::Value, Problem> {
    let name = body.name.trim();
    let Some(phone) = format_phone(&body.phone) else {
        return Err(Problem::field_validation(
            "/phone",
            "invalid_phone",
            "Enter a valid Ugandan mobile number, such as 0787444814 or +256787444814.",
        ));
    };
    if name.is_empty() || name.chars().count() > 200 {
        return Err(Problem::field_validation(
            "/name",
            "invalid_length",
            "Name must contain between 1 and 200 characters.",
        ));
    }
    if body.password.len() < 8 || body.password.len() > 1024 {
        return Err(Problem::field_validation(
            "/password",
            "invalid_length",
            "Password must contain between 8 and 1024 characters.",
        ));
    }
    let email = body
        .email
        .map(|email| email.trim().to_lowercase())
        .filter(|email| !email.is_empty());
    if email
        .as_deref()
        .is_some_and(|email| !email.contains('@') || email.len() > 320)
    {
        return Err(Problem::field_validation(
            "/email",
            "invalid_email",
            "Enter a valid email address or omit email.",
        ));
    }
    let hashed = hash_password(&body.password)
        .await
        .map_err(Problem::internal)?;
    let user_id = Uuid::new_v4();
    let organization_id = Uuid::new_v4();
    let mut tx = state
        .contributions_db
        .begin()
        .await
        .map_err(Problem::internal)?;
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,$2)")
        .bind(organization_id)
        .bind(format!("{}'s organization", name))
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .is_some_and(|db| db.is_unique_violation())
            {
                Problem::conflict()
            } else {
                Problem::internal(error)
            }
        })?;
    sqlx::query("INSERT INTO users(id,organization_id,name,phone,password_hash,role,status,email) VALUES($1,$2,$3,$4,$5,'organization_admin','invited',$6)")
        .bind(user_id).bind(organization_id).bind(name).bind(&phone).bind(hashed).bind(&email).execute(&mut *tx).await.map_err(|error| if error.as_database_error().is_some_and(|db| db.is_unique_violation()) { Problem::conflict() } else { Problem::internal(error) })?;
    tx.commit().await.map_err(Problem::internal)?;
    request_phone_otp(state, &phone).await?;
    Ok(
        serde_json::json!({"id": user_id, "phone": phone, "email": email, "status": "pending_phone_verification"}),
    )
}

pub(crate) async fn request_phone_otp(state: &AppState, raw_phone: &str) -> Result<(), Problem> {
    let phone = format_phone(raw_phone).ok_or_else(|| {
        Problem::field_validation(
            "/phone",
            "invalid_phone",
            "Enter a valid Ugandan mobile number.",
        )
    })?;
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE phone=$1 AND status='invited')",
    )
    .bind(&phone)
    .fetch_one(&state.contributions_db)
    .await
    .map_err(Problem::internal)?;
    if !pending {
        return Err(Problem::new(
            StatusCode::NOT_FOUND,
            "account_not_found",
            "Pending account was not found.",
        ));
    }
    let code = generate_otp(state, "phone", &phone, "signup").await?;
    if let Err(error) = send_sms_otp(state, &phone, &code).await {
        use redis::AsyncCommands;
        let _: usize = state
            .redis
            .clone()
            .del(otp_key("phone", &phone, "signup"))
            .await
            .unwrap_or_default();
        return Err(error);
    }
    Ok(())
}

pub(crate) async fn verify_phone(
    state: &AppState,
    raw_phone: &str,
    code: &str,
) -> Result<(), Problem> {
    let phone = format_phone(raw_phone).ok_or_else(|| {
        Problem::field_validation(
            "/phone",
            "invalid_phone",
            "Enter a valid Ugandan mobile number.",
        )
    })?;
    consume_otp(state, "phone", &phone, "signup", code).await?;
    let updated = sqlx::query("UPDATE users SET phone_verified_at=now(),status='active',updated_at=now(),version=version+1 WHERE phone=$1 AND status='invited'")
        .bind(&phone).execute(&state.contributions_db).await.map_err(Problem::internal)?;
    if updated.rows_affected() == 0 {
        return Err(Problem::new(
            StatusCode::NOT_FOUND,
            "account_not_found",
            "Pending account was not found.",
        ));
    }
    Ok(())
}

pub(crate) async fn request_email_otp(
    state: &AppState,
    user_id: Uuid,
    raw_email: &str,
) -> Result<(), Problem> {
    let email = raw_email.trim().to_lowercase();
    if email.len() > 320 || !email.contains('@') {
        return Err(Problem::field_validation(
            "/email",
            "invalid_email",
            "Enter a valid email address.",
        ));
    }
    let code = generate_otp(state, "email", &email, &user_id.to_string()).await?;
    if let Err(error) = send_email_otp(state, &email, &code).await {
        use redis::AsyncCommands;
        let _: usize = state
            .redis
            .clone()
            .del(otp_key("email", &email, &user_id.to_string()))
            .await
            .unwrap_or_default();
        return Err(error);
    }
    Ok(())
}

pub(crate) async fn verify_email(
    state: &AppState,
    user_id: Uuid,
    raw_email: &str,
    code: &str,
) -> Result<(), Problem> {
    let email = raw_email.trim().to_lowercase();
    consume_otp(state, "email", &email, &user_id.to_string(), code).await?;
    sqlx::query("UPDATE users SET email=$2,email_verified_at=now(),updated_at=now(),version=version+1 WHERE id=$1")
        .bind(user_id).bind(email).execute(&state.contributions_db).await.map_err(|error| if error.as_database_error().is_some_and(|db| db.is_unique_violation()) { Problem::conflict() } else { Problem::internal(error) })?;
    Ok(())
}
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
    let phone = format_phone(phone).ok_or_else(|| anyhow::anyhow!("invalid admin phone"))?;
    anyhow::ensure!(
        !organization.trim().is_empty() && !name.trim().is_empty(),
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
    sqlx::query("INSERT INTO users(id,organization_id,name,phone,password_hash,role,phone_verified_at) VALUES($1,$2,$3,$4,$5,'organization_admin',now())")
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
