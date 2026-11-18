use sea_orm::{ConnectionTrait, DbBackend};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() == DbBackend::Postgres {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                ALTER TABLE users
                    ADD COLUMN IF NOT EXISTS email text,
                    ADD COLUMN IF NOT EXISTS phone_verified_at timestamptz,
                    ADD COLUMN IF NOT EXISTS email_verified_at timestamptz;
                UPDATE users SET phone_verified_at=now() WHERE phone_verified_at IS NULL;
                CREATE UNIQUE INDEX IF NOT EXISTS users_email_lower_unique
                    ON users (lower(email)) WHERE email IS NOT NULL;
                "#,
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() == DbBackend::Postgres {
            manager.get_connection().execute_unprepared(
                "DROP INDEX IF EXISTS users_email_lower_unique; ALTER TABLE users DROP COLUMN IF EXISTS email_verified_at, DROP COLUMN IF EXISTS phone_verified_at, DROP COLUMN IF EXISTS email;",
            ).await?;
        }
        Ok(())
    }
}
