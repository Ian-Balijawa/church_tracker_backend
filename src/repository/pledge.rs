use sqlx::{PgPool, Row};
use uuid::Uuid;

pub(crate) async fn owner(
    db: &PgPool,
    organization_id: Uuid,
    id: Uuid,
) -> Result<Option<(Uuid, Uuid)>, sqlx::Error> {
    let row =
        sqlx::query("SELECT member_id,project_id FROM pledges WHERE id=$1 AND organization_id=$2")
            .bind(id)
            .bind(organization_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|r| (r.get("member_id"), r.get("project_id"))))
}
