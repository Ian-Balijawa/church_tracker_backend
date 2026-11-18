use crate::http::contribution::Problem;
use crate::repository::contribution::{db_error, load_ledger_tx};
use crate::service::contribution_auth::{Principal, member_access, project_access};
use chrono::{DateTime, NaiveDate, Utc};
use contributions_domain::replay;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::service::contribution::*;
pub(crate) async fn full_finance_access(
    db: &PgPool,
    p: &Principal,
    member_id: Uuid,
) -> Result<(), Problem> {
    if !member_access(db, p, member_id).await? {
        return Err(Problem::not_found());
    }
    let projects: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT project_id FROM pledges WHERE organization_id=$1 AND member_id=$2",
    )
    .bind(p.organization_id)
    .bind(member_id)
    .fetch_all(db)
    .await
    .map_err(db_error)?;
    for project in projects {
        if !project_access(db, p, project).await? {
            return Err(Problem::forbidden());
        }
    }
    Ok(())
}

pub(crate) async fn payment_json(db: &PgPool, p: &Principal, id: Uuid) -> Result<Value, Problem> {
    let row=sqlx::query("SELECT id,member_id,project_id,amount,date,method,note,status,void_reason,voided_at,version,created_at,updated_at FROM payments WHERE id=$1 AND organization_id=$2")
        .bind(id).bind(p.organization_id).fetch_optional(db).await.map_err(db_error)?.ok_or_else(Problem::not_found)?;
    let member_id: Uuid = row.get("member_id");
    let project_id: Option<Uuid> = row.get("project_id");
    full_finance_access(db, p, member_id).await?;
    if let Some(project_id) = project_id
        && !project_access(db, p, project_id).await?
    {
        return Err(Problem::not_found());
    }
    let allocations=sqlx::query("SELECT a.id,a.pledge_id,a.period_start,a.period_end,a.amount,a.version,a.created_at,a.updated_at,x.project_id FROM allocations a JOIN pledges x ON x.id=a.pledge_id WHERE a.payment_id=$1 ORDER BY a.period_end,a.period_start,a.pledge_id")
        .bind(id).fetch_all(db).await.map_err(db_error)?;
    let mut alloc_json = Vec::new();
    let mut allocated = 0i64;
    for a in allocations {
        let project_id: Uuid = a.get("project_id");
        if !project_access(db, p, project_id).await? {
            continue;
        }
        let amount: i64 = a.get("amount");
        allocated = allocated
            .checked_add(amount)
            .ok_or_else(|| Problem::internal("overflow"))?;
        alloc_json.push(json!({"id":a.get::<Uuid,_>("id"),"paymentId":id,"pledgeId":a.get::<Uuid,_>("pledge_id"),"periodStart":a.get::<NaiveDate,_>("period_start"),"periodEnd":a.get::<NaiveDate,_>("period_end"),"amount":amount,"version":a.get::<i64,_>("version"),"createdAt":a.get::<DateTime<Utc>,_>("created_at"),"updatedAt":a.get::<DateTime<Utc>,_>("updated_at")}));
    }
    let amount: i64 = row.get("amount");
    let status: String = row.get("status");
    let credit = if status == "voided" {
        0
    } else {
        amount - allocated
    };
    Ok(
        json!({"id":id,"memberId":member_id,"projectId":project_id,"amount":amount,"date":row.get::<NaiveDate,_>("date"),"method":row.get::<String,_>("method"),"note":row.get::<Option<String>,_>("note"),"status":status,"voidReason":row.get::<Option<String>,_>("void_reason"),"voidedAt":row.get::<Option<DateTime<Utc>>,_>("voided_at"),"allocations":alloc_json,"unallocatedCredit":credit,"version":row.get::<i64,_>("version"),"createdAt":row.get::<DateTime<Utc>,_>("created_at"),"updatedAt":row.get::<DateTime<Utc>,_>("updated_at")}),
    )
}

pub(crate) async fn recalculate(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    member: Uuid,
    primary_payment: Option<Uuid>,
) -> Result<contributions_domain::Replay, Problem> {
    let (pledges, payments) = load_ledger_tx(tx, org, member).await?;
    let result = replay(&pledges, &payments, today()).map_err(Problem::internal)?;
    for payment in &payments {
        let old=sqlx::query("SELECT id,pledge_id,period_start,period_end,amount FROM allocations WHERE payment_id=$1 ORDER BY period_end,period_start,pledge_id")
            .bind(payment.id).fetch_all(&mut **tx).await.map_err(db_error)?;
        let mut old_keys = old
            .iter()
            .map(|r| {
                (
                    r.get::<Uuid, _>("pledge_id"),
                    r.get::<NaiveDate, _>("period_start"),
                    r.get::<NaiveDate, _>("period_end"),
                    r.get::<i64, _>("amount"),
                )
            })
            .collect::<Vec<_>>();
        let mut new_keys = result
            .allocations
            .iter()
            .filter(|a| a.payment_id == payment.id)
            .map(|a| (a.pledge_id, a.period_start, a.period_end, a.amount))
            .collect::<Vec<_>>();
        old_keys.sort();
        new_keys.sort();
        if old_keys == new_keys {
            continue;
        }
        sqlx::query("INSERT INTO allocation_history(id,payment_id,pledge_id,period_start,period_end,amount) SELECT id,payment_id,pledge_id,period_start,period_end,amount FROM allocations WHERE payment_id=$1")
            .bind(payment.id).execute(&mut **tx).await.map_err(db_error)?;
        sqlx::query("DELETE FROM allocations WHERE payment_id=$1")
            .bind(payment.id)
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
        for allocation in result
            .allocations
            .iter()
            .filter(|a| a.payment_id == payment.id)
        {
            sqlx::query("INSERT INTO allocations(id,payment_id,pledge_id,period_start,period_end,amount) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(Uuid::new_v4()).bind(allocation.payment_id).bind(allocation.pledge_id).bind(allocation.period_start).bind(allocation.period_end).bind(allocation.amount)
                .execute(&mut **tx).await.map_err(db_error)?;
        }
        if Some(payment.id) != primary_payment {
            sqlx::query("UPDATE payments SET version=version+1,updated_at=now() WHERE id=$1")
                .bind(payment.id)
                .execute(&mut **tx)
                .await
                .map_err(db_error)?;
        }
    }
    sqlx::query(
        "UPDATE members SET version=version+1,updated_at=now() WHERE id=$1 AND organization_id=$2",
    )
    .bind(member)
    .bind(org)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    sqlx::query("UPDATE projects SET version=version+1,updated_at=now() WHERE organization_id=$1 AND id IN (SELECT DISTINCT project_id FROM pledges WHERE member_id=$2)")
        .bind(org).bind(member).execute(&mut **tx).await.map_err(db_error)?;
    Ok(result)
}

pub(crate) async fn payment_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    credit: i64,
) -> Result<Value, Problem> {
    let row=sqlx::query("SELECT member_id,project_id,amount,date,method,note,status,void_reason,voided_at,version,created_at,updated_at FROM payments WHERE id=$1")
        .bind(id).fetch_one(&mut **tx).await.map_err(db_error)?;
    let allocation_rows=sqlx::query("SELECT id,pledge_id,period_start,period_end,amount,version,created_at,updated_at FROM allocations WHERE payment_id=$1 ORDER BY period_end,period_start,pledge_id")
        .bind(id).fetch_all(&mut **tx).await.map_err(db_error)?;
    let allocations=allocation_rows.into_iter().map(|a|json!({"id":a.get::<Uuid,_>("id"),"paymentId":id,"pledgeId":a.get::<Uuid,_>("pledge_id"),"periodStart":a.get::<NaiveDate,_>("period_start"),"periodEnd":a.get::<NaiveDate,_>("period_end"),"amount":a.get::<i64,_>("amount"),"version":a.get::<i64,_>("version"),"createdAt":a.get::<DateTime<Utc>,_>("created_at"),"updatedAt":a.get::<DateTime<Utc>,_>("updated_at")})).collect::<Vec<_>>();
    Ok(
        json!({"id":id,"memberId":row.get::<Uuid,_>("member_id"),"projectId":row.get::<Option<Uuid>,_>("project_id"),"amount":row.get::<i64,_>("amount"),"date":row.get::<NaiveDate,_>("date"),"method":row.get::<String,_>("method"),"note":row.get::<Option<String>,_>("note"),"status":row.get::<String,_>("status"),"voidReason":row.get::<Option<String>,_>("void_reason"),"voidedAt":row.get::<Option<DateTime<Utc>>,_>("voided_at"),"allocations":allocations,"unallocatedCredit":credit,"version":row.get::<i64,_>("version"),"createdAt":row.get::<DateTime<Utc>,_>("created_at"),"updatedAt":row.get::<DateTime<Utc>,_>("updated_at")}),
    )
}
