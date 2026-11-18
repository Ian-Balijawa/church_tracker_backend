use crate::http::contribution::Problem;
use crate::repository::contribution::{db_error, load_ledger};
use crate::service::contribution_auth::{Principal, member_access, project_access};
use chrono::{DateTime, NaiveDate, Utc};
use contributions_domain::{Frequency, StateAction, replay};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub(crate) async fn pledge_json(
    db: &PgPool,
    p: &Principal,
    id: Uuid,
    as_of: NaiveDate,
) -> Result<Value, Problem> {
    let row=sqlx::query("SELECT id,member_id,project_id,frequency,start_date,end_date,status,version,created_at,updated_at FROM pledges WHERE id=$1 AND organization_id=$2")
        .bind(id).bind(p.organization_id).fetch_optional(db).await.map_err(db_error)?.ok_or_else(Problem::not_found)?;
    let member_id: Uuid = row.get("member_id");
    let project_id: Uuid = row.get("project_id");
    if !member_access(db, p, member_id).await? || !project_access(db, p, project_id).await? {
        return Err(Problem::not_found());
    }
    let versions=sqlx::query("SELECT id,amount,effective_from,created_at FROM pledge_versions WHERE pledge_id=$1 ORDER BY effective_from")
        .bind(id).fetch_all(db).await.map_err(db_error)?.into_iter().map(|v|json!({"id":v.get::<Uuid,_>("id"),"amount":v.get::<i64,_>("amount"),"effectiveFrom":v.get::<NaiveDate,_>("effective_from"),"createdAt":v.get::<DateTime<Utc>,_>("created_at")})).collect::<Vec<_>>();
    let (pledges, payments) = load_ledger(db, p.organization_id, member_id).await?;
    let result = replay(&pledges, &payments, as_of).map_err(Problem::internal)?;
    let b = result.pledge_balances.get(&id).cloned().unwrap_or_default();
    Ok(
        json!({"id":id,"memberId":member_id,"projectId":project_id,"frequency":row.get::<String,_>("frequency"),"startDate":row.get::<NaiveDate,_>("start_date"),"endDate":row.get::<Option<NaiveDate>,_>("end_date"),"status":row.get::<String,_>("status"),"versions":versions,"demand":b.demand,"paid":b.paid,"debt":b.debt,"dueToday":b.due_today,"advance":b.advance,"version":row.get::<i64,_>("version"),"createdAt":row.get::<DateTime<Utc>,_>("created_at"),"updatedAt":row.get::<DateTime<Utc>,_>("updated_at")}),
    )
}

pub(crate) fn parse_frequency(value: &str) -> Result<Frequency, Problem> {
    match value {
        "daily" => Ok(Frequency::Daily),
        "weekly" => Ok(Frequency::Weekly),
        "monthly" => Ok(Frequency::Monthly),
        _ => Err(Problem::field_validation(
            "/frequency",
            "invalid_frequency",
            "Frequency must be daily, weekly, or monthly.",
        )),
    }
}

pub(crate) fn parse_action(value: &str) -> Result<StateAction, Problem> {
    match value {
        "pause" => Ok(StateAction::Pause),
        "resume" => Ok(StateAction::Resume),
        "cancel" => Ok(StateAction::Cancel),
        _ => Err(Problem::internal("invalid pledge event")),
    }
}
