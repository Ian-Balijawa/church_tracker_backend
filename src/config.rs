use std::env;

#[derive(Clone)]
pub struct Config {
    pub db_url: String,
    pub redis_url: String,
    pub log_level: String,
    pub cors_origins: Vec<String>,
    pub app_env: String,
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
        })
    }
}
