use crate::http::contribution::Problem;
use crate::repository::contribution::{db_error, load_ledger};
use crate::service::contribution_auth::{Principal, member_access, project_access};
use chrono::{DateTime, NaiveDate, Utc};
use contributions_domain::replay;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub(crate) async fn project_json(
    db: &PgPool,
    p: &Principal,
    id: Uuid,
    as_of: NaiveDate,
) -> Result<Value, Problem> {
    if !project_access(db, p, id).await? {
        return Err(Problem::not_found());
    }
    let row=sqlx::query("SELECT id,name,description,owner_church_id,target,start_date,end_date,status,version,created_at,updated_at FROM projects WHERE id=$1 AND organization_id=$2 AND deleted_at IS NULL")
        .bind(id).bind(p.organization_id).fetch_optional(db).await.map_err(db_error)?.ok_or_else(Problem::not_found)?;
    let church_ids:Vec<Uuid>=sqlx::query_scalar("SELECT church_id FROM project_churches WHERE project_id=$1 AND detached_at IS NULL ORDER BY church_id").bind(id).fetch_all(db).await.map_err(db_error)?;
    let member_count:i64=sqlx::query_scalar("SELECT count(*) FROM project_members pm JOIN members m ON m.id=pm.member_id WHERE pm.project_id=$1 AND pm.detached_at IS NULL AND m.deleted_at IS NULL AND ($2='organization_admin' OR EXISTS(SELECT 1 FROM user_churches uc WHERE uc.user_id=$3 AND (uc.church_id=m.home_church_id OR EXISTS(SELECT 1 FROM member_churches mc WHERE mc.member_id=m.id AND mc.church_id=uc.church_id AND mc.detached_at IS NULL))))")
        .bind(id).bind(&p.role).bind(p.id).fetch_one(db).await.map_err(db_error)?;
    let member_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT member_id FROM pledges WHERE project_id=$1 AND organization_id=$2",
    )
    .bind(id)
    .bind(p.organization_id)
    .fetch_all(db)
    .await
    .map_err(db_error)?;
    let mut collected = 0i64;
    let mut debt = 0i64;
    let mut due_today = 0i64;
    let mut advance = 0i64;
    for member_id in member_ids {
        if !member_access(db, p, member_id).await? {
            continue;
        }
        let (pledges, payments) = load_ledger(db, p.organization_id, member_id).await?;
        let calc = replay(&pledges, &payments, as_of).map_err(Problem::internal)?;
        for pledge in pledges.iter().filter(|pledge| pledge.project_id == id) {
            if let Some(b) = calc.pledge_balances.get(&pledge.id) {
                debt = debt
                    .checked_add(b.debt)
                    .ok_or_else(|| Problem::internal("overflow"))?;
                due_today = due_today
                    .checked_add(b.due_today)
                    .ok_or_else(|| Problem::internal("overflow"))?;
                advance = advance
                    .checked_add(b.advance)
                    .ok_or_else(|| Problem::internal("overflow"))?;
            }
        }
        for allocation in calc.allocations.iter().filter(|a| {
            pledges
                .iter()
                .any(|pledge| pledge.id == a.pledge_id && pledge.project_id == id)
        }) {
            collected = collected
                .checked_add(allocation.amount)
                .ok_or_else(|| Problem::internal("overflow"))?;
        }
    }
    let target: i64 = row.get("target");
    let progress: Option<i64> = if target == 0 {
        None
    } else {
        Some(
            (i128::from(collected) * 10_000 / i128::from(target))
                .try_into()
                .map_err(|_| Problem::internal("progress overflow"))?,
        )
    };
    Ok(
        json!({"id":id,"name":row.get::<String,_>("name"),"description":row.get::<Option<String>,_>("description"),"ownerChurchId":row.get::<Uuid,_>("owner_church_id"),"target":target,"startDate":row.get::<NaiveDate,_>("start_date"),"endDate":row.get::<Option<NaiveDate>,_>("end_date"),"status":row.get::<String,_>("status"),"churchIds":church_ids,"memberCount":member_count,"collected":collected,"debt":debt,"dueToday":due_today,"advance":advance,"targetProgressBasisPoints":progress,"version":row.get::<i64,_>("version"),"createdAt":row.get::<DateTime<Utc>,_>("created_at"),"updatedAt":row.get::<DateTime<Utc>,_>("updated_at")}),
    )
}
