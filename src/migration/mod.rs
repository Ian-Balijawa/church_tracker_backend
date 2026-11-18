mod m20261117_000005_contributions;
mod m20261118_000006_auth_verification;
use sea_orm_migration::prelude::*;
pub struct Migrator;
#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261117_000005_contributions::Migration),
            Box::new(m20261118_000006_auth_verification::Migration),
        ]
    }
}
