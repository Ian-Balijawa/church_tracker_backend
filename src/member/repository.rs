use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(crate) async fn status_for_update(
    tx: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT status FROM members WHERE id=$1 AND organization_id=$2 FOR UPDATE")
        .bind(id)
        .bind(organization_id)
        .fetch_optional(&mut **tx)
        .await
}
