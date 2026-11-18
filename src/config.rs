use std::env;

#[derive(Clone)]
pub struct Config {
    pub db_url: String,
    pub redis_url: String,
    pub log_level: String,
    pub cors_origins: Vec<String>,
    pub app_env: String,
    pub sms_username: Option<String>,
    pub sms_password: Option<String>,
    pub sms_sender_id: Option<String>,
    pub smtp_server: String,
    pub smtp_port: u16,
    pub smtp_user: String,
    pub smtp_pass: String,
    pub jwt_secret: String,
    pub jwt_issuer: String,
    pub jwt_audience: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let db_url = env::var("DATABASE_URL")?;
        anyhow::ensure!(
            db_url.starts_with("postgres://") || db_url.starts_with("postgresql://"),
            "DATABASE_URL must use PostgreSQL"
        );
        let redis_url = env::var("REDIS_URL")?;
        anyhow::ensure!(
            redis_url.starts_with("redis://") || redis_url.starts_with("rediss://"),
            "REDIS_URL must use Redis"
        );
        let cors_origins = env::var("CORS_ORIGINS")?
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        anyhow::ensure!(!cors_origins.is_empty(), "CORS_ORIGINS cannot be empty");
        for origin in &cors_origins {
            anyhow::ensure!(
                origin.starts_with("https://")
                    || origin.starts_with("http://localhost:")
                    || origin.starts_with("http://127.0.0.1:"),
                "CORS_ORIGINS requires HTTPS or localhost"
            );
        }
        let app_env = env::var("APP_ENV").unwrap_or_else(|_| "development".to_owned());
        anyhow::ensure!(
            matches!(app_env.as_str(), "development" | "production"),
            "APP_ENV must be development or production"
        );
        Ok(Self {
            db_url,
            redis_url,
            log_level: env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned()),
            cors_origins,
            app_env,
            sms_username: env::var("SMS_USERNAME").ok(),
            sms_password: env::var("SMS_PASSWORD").ok(),
            sms_sender_id: env::var("SMS_SENDER_ID").ok(),
            smtp_server: env::var("SMTP_SERVER").unwrap_or_else(|_| "smtp.gmail.com".to_owned()),
            smtp_port: env::var("SMTP_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(587),
            smtp_user: env::var("SMTP_USER")
                .or_else(|_| env::var("SENDER_EMAIL"))
                .unwrap_or_default(),
            smtp_pass: env::var("SMTP_PASSWORD")
                .or_else(|_| env::var("APP_PASSWORD"))
                .unwrap_or_default(),
            jwt_secret: env::var("JWT_SECRET")
                .unwrap_or_else(|_| "development-only-change-this-jwt-secret".to_owned()),
            jwt_issuer: env::var("JWT_ISSUER")
                .unwrap_or_else(|_| "church-contributions-api".to_owned()),
            jwt_audience: env::var("JWT_AUDIENCE")
                .unwrap_or_else(|_| "church-contributions-client".to_owned()),
        })
    }
}
