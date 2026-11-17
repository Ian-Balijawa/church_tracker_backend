use crate::http::contribution::Problem;
use crate::repository::contribution::{db_error, load_ledger};
use crate::service::contribution_auth::{Principal, church_access, member_access, project_access};
use chrono::{DateTime, NaiveDate, Utc};
use contributions_domain::{Balance, replay};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub(crate) async fn member_json(
    db: &PgPool,
    p: &Principal,
    id: Uuid,
    as_of: NaiveDate,
) -> Result<Value, Problem> {
    if !member_access(db, p, id).await? {
        return Err(Problem::not_found());
    }
    let row=sqlx::query("SELECT id,name,phone,home_church_id,status,version,created_at,updated_at FROM members WHERE id=$1 AND organization_id=$2 AND deleted_at IS NULL")
        .bind(id).bind(p.organization_id).fetch_optional(db).await.map_err(db_error)?.ok_or_else(Problem::not_found)?;
    let (pledges, payments) = load_ledger(db, p.organization_id, id).await?;
    let calculation = replay(&pledges, &payments, as_of).map_err(|error| {
        tracing::error!(%error,"ledger replay failed");
        Problem::internal(error)
    })?;
    let scoped = if p.role == "organization_admin" {
        calculation.balance.clone()
    } else {
        let mut b = Balance::default();
        for pledge in &pledges {
            if project_access(db, p, pledge.project_id).await? {
                if let Some(item) = calculation.pledge_balances.get(&pledge.id) {
                    b.demand = b
                        .demand
                        .checked_add(item.demand)
                        .ok_or_else(|| Problem::internal("overflow"))?;
                    b.paid = b
                        .paid
                        .checked_add(item.paid)
                        .ok_or_else(|| Problem::internal("overflow"))?;
                    b.debt = b
                        .debt
                        .checked_add(item.debt)
                        .ok_or_else(|| Problem::internal("overflow"))?;
                    b.due_today = b
                        .due_today
                        .checked_add(item.due_today)
                        .ok_or_else(|| Problem::internal("overflow"))?;
                    b.advance = b
                        .advance
                        .checked_add(item.advance)
                        .ok_or_else(|| Problem::internal("overflow"))?;
                }
            }
        }
        b
    };
    let all_church_ids:Vec<Uuid>=sqlx::query_scalar("SELECT church_id FROM member_churches WHERE member_id=$1 AND detached_at IS NULL ORDER BY church_id").bind(id).fetch_all(db).await.map_err(db_error)?;
    let all_project_ids:Vec<Uuid>=sqlx::query_scalar("SELECT project_id FROM project_members WHERE member_id=$1 AND detached_at IS NULL ORDER BY project_id").bind(id).fetch_all(db).await.map_err(db_error)?;
    let mut church_ids = Vec::new();
    for church in all_church_ids {
        if church_access(db, p, church).await? {
            church_ids.push(church);
        }
    }
    let mut project_ids = Vec::new();
    for project in all_project_ids {
        if project_access(db, p, project).await? {
            project_ids.push(project);
        }
    }
    let total_given: i64 = payments
        .iter()
        .filter(|payment| !payment.voided && payment.date <= as_of)
        .try_fold(0i64, |sum, payment| sum.checked_add(payment.amount))
        .ok_or_else(|| Problem::internal("money overflow"))?;
    let mut scoped_total_given = 0i64;
    if p.role != "organization_admin" {
        for allocation in &calculation.allocations {
            if let Some(pledge) = pledges
                .iter()
                .find(|pledge| pledge.id == allocation.pledge_id)
            {
                if project_access(db, p, pledge.project_id).await? {
                    scoped_total_given = scoped_total_given
                        .checked_add(allocation.amount)
                        .ok_or_else(|| Problem::internal("money overflow"))?;
                }
            }
        }
    }
    Ok(
        json!({"id":id,"name":row.get::<String,_>("name"),"phone":row.get::<String,_>("phone"),"homeChurchId":row.get::<Uuid,_>("home_church_id"),"status":row.get::<String,_>("status"),"churchIds":church_ids,"projectIds":project_ids,"totalGiven":if p.role=="organization_admin" {total_given} else {scoped_total_given},"debt":scoped.debt,"dueToday":scoped.due_today,"advance":scoped.advance,"unallocatedCredit":scoped.unallocated_credit,"scoped":p.role!="organization_admin","version":row.get::<i64,_>("version"),"createdAt":row.get::<DateTime<Utc>,_>("created_at"),"updatedAt":row.get::<DateTime<Utc>,_>("updated_at")}),
    )
}
