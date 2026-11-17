use sqlx::PgPool;
use uuid::Uuid;

pub(crate) async fn church_ids(db: &PgPool, user_id: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT church_id FROM user_churches WHERE user_id=$1 ORDER BY church_id")
        .bind(user_id)
        .fetch_all(db)
        .await
}
