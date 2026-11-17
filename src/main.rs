mod bootstrap;
mod config;
mod controller;
mod http;
mod migration;
mod model;
mod openapi;
mod repository;
mod routes;
mod service;

pub use bootstrap::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::run().await
}
