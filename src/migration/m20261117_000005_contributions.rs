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
        manager.get_connection().execute_unprepared(r#"
            CREATE TABLE organizations (
                id uuid PRIMARY KEY,
                name text NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 200),
                created_at timestamptz NOT NULL DEFAULT now()
            );
            CREATE TABLE users (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                name text NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 200),
                phone text NOT NULL UNIQUE,
                password_hash text NOT NULL,
                pin_hash text,
                role text NOT NULL CHECK (role IN ('organization_admin','church_admin','treasurer','viewer')),
                status text NOT NULL DEFAULT 'active' CHECK (status IN ('invited','active','disabled')),
                avatar_url text,
                version bigint NOT NULL DEFAULT 1,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now()
            );
            CREATE TABLE user_churches (
                user_id uuid NOT NULL REFERENCES users(id),
                church_id uuid NOT NULL,
                PRIMARY KEY (user_id, church_id)
            );
            CREATE TABLE sessions (
                id uuid PRIMARY KEY,
                user_id uuid NOT NULL REFERENCES users(id),
                token_hash text NOT NULL UNIQUE,
                csrf_token text NOT NULL,
                device_label text,
                created_at timestamptz NOT NULL DEFAULT now(),
                last_seen_at timestamptz NOT NULL DEFAULT now(),
                expires_at timestamptz NOT NULL,
                revoked_at timestamptz
            );
            CREATE TABLE churches (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                name text NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 200),
                phone text,
                status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
                version bigint NOT NULL DEFAULT 1,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now(),
                deleted_at timestamptz,
                UNIQUE (organization_id, id)
            );
            ALTER TABLE user_churches ADD CONSTRAINT user_churches_church_fk FOREIGN KEY (church_id) REFERENCES churches(id);
            CREATE TABLE members (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                name text NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 200),
                phone text NOT NULL,
                home_church_id uuid NOT NULL REFERENCES churches(id),
                status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
                version bigint NOT NULL DEFAULT 1,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now(),
                deleted_at timestamptz,
                UNIQUE (organization_id, id)
            );
            CREATE TABLE projects (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                name text NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 200),
                description text,
                owner_church_id uuid NOT NULL REFERENCES churches(id),
                target bigint NOT NULL CHECK (target BETWEEN 0 AND 9007199254740991),
                start_date date NOT NULL,
                end_date date,
                status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
                version bigint NOT NULL DEFAULT 1,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now(),
                deleted_at timestamptz,
                CHECK (end_date IS NULL OR end_date >= start_date),
                UNIQUE (organization_id, id)
            );
            CREATE TABLE member_churches (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                member_id uuid NOT NULL REFERENCES members(id),
                church_id uuid NOT NULL REFERENCES churches(id),
                created_at timestamptz NOT NULL DEFAULT now(),
                created_by uuid NOT NULL REFERENCES users(id),
                detached_at timestamptz,
                version bigint NOT NULL DEFAULT 1
            );
            CREATE UNIQUE INDEX member_church_active ON member_churches(member_id,church_id) WHERE detached_at IS NULL;
            CREATE TABLE project_churches (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                project_id uuid NOT NULL REFERENCES projects(id),
                church_id uuid NOT NULL REFERENCES churches(id),
                created_at timestamptz NOT NULL DEFAULT now(),
                created_by uuid NOT NULL REFERENCES users(id),
                detached_at timestamptz,
                version bigint NOT NULL DEFAULT 1
            );
            CREATE UNIQUE INDEX project_church_active ON project_churches(project_id,church_id) WHERE detached_at IS NULL;
            CREATE TABLE project_members (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                project_id uuid NOT NULL REFERENCES projects(id),
                member_id uuid NOT NULL REFERENCES members(id),
                created_at timestamptz NOT NULL DEFAULT now(),
                created_by uuid NOT NULL REFERENCES users(id),
                detached_at timestamptz,
                version bigint NOT NULL DEFAULT 1
            );
            CREATE UNIQUE INDEX project_member_active ON project_members(project_id,member_id) WHERE detached_at IS NULL;
            CREATE TABLE pledges (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                member_id uuid NOT NULL REFERENCES members(id),
                project_id uuid NOT NULL REFERENCES projects(id),
                frequency text NOT NULL CHECK (frequency IN ('daily','weekly','monthly')),
                start_date date NOT NULL,
                end_date date,
                status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','paused','cancelled')),
                version bigint NOT NULL DEFAULT 1,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now(),
                CHECK (end_date IS NULL OR end_date >= start_date)
            );
            CREATE TABLE pledge_versions (
                id uuid PRIMARY KEY,
                pledge_id uuid NOT NULL REFERENCES pledges(id),
                amount bigint NOT NULL CHECK (amount BETWEEN 1 AND 9007199254740991),
                effective_from date NOT NULL,
                created_at timestamptz NOT NULL DEFAULT now(),
                UNIQUE (pledge_id,effective_from)
            );
            CREATE TABLE pledge_events (
                id uuid PRIMARY KEY,
                pledge_id uuid NOT NULL REFERENCES pledges(id),
                action text NOT NULL CHECK (action IN ('pause','resume','cancel')),
                effective_date date NOT NULL,
                reason text,
                created_by uuid NOT NULL REFERENCES users(id),
                created_at timestamptz NOT NULL DEFAULT now(),
                UNIQUE (pledge_id,effective_date)
            );
            CREATE TABLE payments (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL REFERENCES organizations(id),
                member_id uuid NOT NULL REFERENCES members(id),
                project_id uuid REFERENCES projects(id),
                amount bigint NOT NULL CHECK (amount BETWEEN 1 AND 9007199254740991),
                date date NOT NULL,
                method text NOT NULL CHECK (method IN ('cash','mobile_money','bank')),
                note text,
                status text NOT NULL DEFAULT 'recorded' CHECK (status IN ('recorded','voided')),
                void_reason text,
                voided_at timestamptz,
                voided_by uuid REFERENCES users(id),
                version bigint NOT NULL DEFAULT 1,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now()
            );
            CREATE TABLE allocations (
                id uuid PRIMARY KEY,
                payment_id uuid NOT NULL REFERENCES payments(id),
                pledge_id uuid NOT NULL REFERENCES pledges(id),
                period_start date NOT NULL,
                period_end date NOT NULL,
                amount bigint NOT NULL CHECK (amount BETWEEN 1 AND 9007199254740991),
                version bigint NOT NULL DEFAULT 1,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now()
            );
            CREATE TABLE allocation_history (
                id uuid NOT NULL,
                payment_id uuid NOT NULL,
                pledge_id uuid NOT NULL,
                period_start date NOT NULL,
                period_end date NOT NULL,
                amount bigint NOT NULL,
                replaced_at timestamptz NOT NULL DEFAULT now()
            );
            CREATE INDEX payments_member_date ON payments(organization_id,member_id,date,id);
            CREATE INDEX pledges_member ON pledges(organization_id,member_id);
            CREATE TABLE idempotency (
                organization_id uuid NOT NULL,
                user_id uuid NOT NULL,
                key uuid NOT NULL,
                fingerprint text NOT NULL,
                status integer NOT NULL,
                response jsonb NOT NULL,
                location text,
                etag text,
                created_at timestamptz NOT NULL DEFAULT now(),
                PRIMARY KEY (organization_id,user_id,key)
            );
            CREATE TABLE audit (
                id uuid PRIMARY KEY,
                organization_id uuid NOT NULL,
                actor_id uuid,
                action text NOT NULL,
                entity_type text NOT NULL,
                entity_id uuid NOT NULL,
                occurred_at timestamptz NOT NULL DEFAULT now(),
                request_id uuid NOT NULL,
                before jsonb,
                after jsonb,
                reason text
            );
        "#).await?;
        Ok(())
    }
}
