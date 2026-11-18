mod auth;
mod bootstrap;
mod church;
mod config;
mod contribution;
mod controller;
mod health;
mod http;
mod member;
mod migration;
mod model;
mod openapi;
mod payment;
mod pledge;
mod project;
mod repository;
mod routes;
mod service;

pub use bootstrap::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::run().await
}
