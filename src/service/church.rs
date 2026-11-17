use crate::http::contribution::Problem;
use crate::repository::contribution::db_error;
use crate::service::contribution_auth::{Principal, church_access};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub(crate) async fn church_json(db: &PgPool, p: &Principal, id: Uuid) -> Result<Value, Problem> {
    if !church_access(db, p, id).await? {
        return Err(Problem::not_found());
    }
    let row = sqlx::query("SELECT c.id,c.name,c.phone,c.status,c.version,c.created_at,c.updated_at,(SELECT count(*) FROM members m WHERE m.home_church_id=c.id AND m.deleted_at IS NULL) AS member_count,(SELECT count(*) FROM projects x WHERE x.owner_church_id=c.id AND x.deleted_at IS NULL) AS project_count FROM churches c WHERE c.id=$1 AND c.organization_id=$2 AND c.deleted_at IS NULL")
        .bind(id).bind(p.organization_id).fetch_optional(db).await.map_err(db_error)?.ok_or_else(Problem::not_found)?;
    Ok(
        json!({"id":id,"name":row.get::<String,_>("name"),"phone":row.get::<Option<String>,_>("phone"),"status":row.get::<String,_>("status"),"memberCount":row.get::<i64,_>("member_count"),"projectCount":row.get::<i64,_>("project_count"),"version":row.get::<i64,_>("version"),"createdAt":row.get::<DateTime<Utc>,_>("created_at"),"updatedAt":row.get::<DateTime<Utc>,_>("updated_at")}),
    )
}
