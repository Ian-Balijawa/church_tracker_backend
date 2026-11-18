use sea_orm::{ConnectionTrait, DbBackend};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Postgres {
            return Ok(());
        }
        manager.get_connection().execute_unprepared(
            r#"
            CREATE TABLE payment_collections (
                id uuid PRIMARY KEY,
                reference uuid NOT NULL UNIQUE,
                external_uuid text UNIQUE,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                initiated_by uuid NOT NULL REFERENCES users(id),
                member_id uuid NOT NULL REFERENCES members(id),
                project_id uuid REFERENCES projects(id),
                amount bigint NOT NULL CHECK (amount BETWEEN 500 AND 10000000),
                currency text NOT NULL DEFAULT 'UGX' CHECK (currency='UGX'),
                phone_number text NOT NULL CHECK (phone_number ~ '^\+256[0-9]{9}$'),
                provider text NOT NULL CHECK (provider IN ('mtn','airtel')),
                status text NOT NULL DEFAULT 'initiating'
                    CHECK (status IN ('initiating','pending','processing','successful','failed','cancelled','sandbox')),
                mode text,
                description text CHECK (description IS NULL OR char_length(description) <= 255),
                provider_reference text,
                provider_transaction_id text,
                failure_reason text,
                request_fingerprint text NOT NULL,
                idempotency_key uuid NOT NULL,
                provider_response jsonb,
                webhook_payload jsonb,
                webhook_attempts integer NOT NULL DEFAULT 0 CHECK (webhook_attempts >= 0),
                last_webhook_at timestamptz,
                initiated_at timestamptz NOT NULL DEFAULT now(),
                completed_at timestamptz,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now(),
                UNIQUE (organization_id,initiated_by,idempotency_key)
            );
            CREATE INDEX payment_collections_member_created_idx
                ON payment_collections (organization_id,member_id,created_at DESC);
            CREATE INDEX payment_collections_status_created_idx
                ON payment_collections (status,created_at);
            ALTER TABLE payments
                ADD COLUMN collection_id uuid UNIQUE REFERENCES payment_collections(id);
            "#,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() == DbBackend::Postgres {
            manager
                .get_connection()
                .execute_unprepared(
                    "ALTER TABLE payments DROP COLUMN IF EXISTS collection_id; DROP TABLE IF EXISTS payment_collections;",
                )
                .await?;
        }
        Ok(())
    }
}
