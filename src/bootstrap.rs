use crate::{config::Config, migration, routes, service};
use axum::http::{HeaderValue, Method, header};
use clap::{Parser, Subcommand};
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tower_http::{
    cors::CorsLayer,
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

#[derive(Clone)]
pub struct AppState {
    pub contributions_db: sqlx::PgPool,
    pub redis: redis::aio::ConnectionManager,
    pub config: Config,
}

#[derive(Parser)]
#[command(name = "church-contributions-api")]
struct Cli {
    #[arg(long, default_value = "0.0.0.0")]
    bind_ip: IpAddr,
    #[arg(long, default_value_t = 8000)]
    port: u16,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Migrate,
    CreateAdmin {
        #[arg(long)]
        organization: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        phone: String,
        #[arg(long)]
        password: String,
    },
}

pub async fn run() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let config = Config::from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&config.log_level))
        .init();
    let migration_db = Database::connect(&config.db_url).await?;
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.db_url)
        .await?;
    match cli.command {
        Some(Command::Migrate) => {
            migration::Migrator::up(&migration_db, None).await?;
            return Ok(());
        }
        Some(Command::CreateAdmin {
            organization,
            name,
            phone,
            password,
        }) => {
            anyhow::ensure!(
                migration::Migrator::get_pending_migrations_read_only(&migration_db)
                    .await?
                    .is_empty(),
                "run `make migrate` first"
            );
            service::contribution_auth::create_admin(&db, &organization, &name, &phone, &password)
                .await?;
            return Ok(());
        }
        None => {}
    }
    anyhow::ensure!(
        migration::Migrator::get_pending_migrations_read_only(&migration_db)
            .await?
            .is_empty(),
        "database migrations are pending; run `make migrate`"
    );
    let redis_client = redis::Client::open(config.redis_url.as_str())?;
    let mut redis = redis::aio::ConnectionManager::new(redis_client).await?;
    let _: String = redis::cmd("PING").query_async(&mut redis).await?;
    let allowed_origins = config
        .cors_origins
        .iter()
        .map(|s| HeaderValue::from_str(s))
        .collect::<Result<Vec<_>, _>>()?;
    let state = AppState {
        contributions_db: db,
        redis,
        config,
    };
    let app = routes::create_router()
        .with_state(state)
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(allowed_origins)
                .allow_credentials(true)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PATCH,
                    Method::PUT,
                    Method::DELETE,
                ])
                .allow_headers([
                    header::CONTENT_TYPE,
                    header::IF_MATCH,
                    header::HeaderName::from_static("x-csrf-token"),
                    header::HeaderName::from_static("idempotency-key"),
                ]),
        )
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http());
    let address = SocketAddr::new(cli.bind_ip, cli.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
