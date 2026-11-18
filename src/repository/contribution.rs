use crate::http::contribution::{Problem, no_store};
use crate::service::contribution_auth::Principal;
use crate::service::pledge::{parse_action, parse_frequency};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use contributions_domain::{
    AmountVersion, Payment as DomainPayment, Pledge as DomainPledge, StateEvent,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

pub(crate) async fn load_ledger_tx(
    tx: &mut Transaction<'_, Postgres>,
    org: Uuid,
    member: Uuid,
) -> Result<(Vec<DomainPledge>, Vec<DomainPayment>), Problem> {
    let rows=sqlx::query("SELECT id,project_id,frequency,start_date,end_date FROM pledges WHERE organization_id=$1 AND member_id=$2")
        .bind(org).bind(member).fetch_all(&mut **tx).await.map_err(db_error)?;
    let mut pledges = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.get("id");
        let versions=sqlx::query("SELECT id,amount,effective_from FROM pledge_versions WHERE pledge_id=$1 ORDER BY effective_from")
            .bind(id).fetch_all(&mut **tx).await.map_err(db_error)?.into_iter().map(|r|AmountVersion{id:r.get("id"),amount:r.get("amount"),effective_from:r.get("effective_from")}).collect();
        let events=sqlx::query("SELECT action,effective_date FROM pledge_events WHERE pledge_id=$1 ORDER BY effective_date")
            .bind(id).fetch_all(&mut **tx).await.map_err(db_error)?.into_iter().map(|e|Ok(StateEvent{action:parse_action(e.get::<&str,_>("action"))?,effective_date:e.get("effective_date")})).collect::<Result<Vec<_>,Problem>>()?;
        pledges.push(DomainPledge {
            id,
            project_id: row.get("project_id"),
            frequency: parse_frequency(row.get::<&str, _>("frequency"))?,
            start_date: row.get("start_date"),
            end_date: row.get("end_date"),
            versions,
            events,
        });
    }
    let payments=sqlx::query("SELECT id,project_id,amount,date,status FROM payments WHERE organization_id=$1 AND member_id=$2")
        .bind(org).bind(member).fetch_all(&mut **tx).await.map_err(db_error)?.into_iter().map(|r|DomainPayment{id:r.get("id"),project_id:r.get("project_id"),amount:r.get("amount"),date:r.get("date"),voided:r.get::<&str,_>("status")=="voided"}).collect();
    Ok((pledges, payments))
}
pub(crate) async fn save_replay(
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    key: Uuid,
    status: StatusCode,
    response: &Value,
    location: Option<&str>,
    etag: Option<&str>,
) -> Result<(), Problem> {
    sqlx::query("UPDATE idempotency SET status=$4,response=$5,location=$6,etag=$7 WHERE organization_id=$1 AND user_id=$2 AND key=$3")
        .bind(p.organization_id).bind(p.id).bind(key).bind(i32::from(status.as_u16())).bind(response).bind(location).bind(etag)
        .execute(&mut **tx).await.map_err(db_error)?;
    Ok(())
}
pub(crate) async fn load_ledger(
    db: &PgPool,
    organization_id: Uuid,
    member_id: Uuid,
) -> Result<(Vec<DomainPledge>, Vec<DomainPayment>), Problem> {
    let rows=sqlx::query("SELECT id,project_id,frequency,start_date,end_date FROM pledges WHERE organization_id=$1 AND member_id=$2")
        .bind(organization_id).bind(member_id).fetch_all(db).await.map_err(db_error)?;
    let mut pledges = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.get("id");
        let versions=sqlx::query("SELECT id,amount,effective_from FROM pledge_versions WHERE pledge_id=$1 ORDER BY effective_from")
            .bind(id).fetch_all(db).await.map_err(db_error)?.into_iter().map(|v| AmountVersion{id:v.get("id"),amount:v.get("amount"),effective_from:v.get("effective_from")}).collect();
        let events=sqlx::query("SELECT action,effective_date FROM pledge_events WHERE pledge_id=$1 ORDER BY effective_date")
            .bind(id).fetch_all(db).await.map_err(db_error)?.into_iter().map(|e|Ok(StateEvent{action:parse_action(e.get::<&str,_>("action"))?,effective_date:e.get("effective_date")})).collect::<Result<Vec<_>,Problem>>()?;
        pledges.push(DomainPledge {
            id,
            project_id: row.get("project_id"),
            frequency: parse_frequency(row.get::<&str, _>("frequency"))?,
            start_date: row.get("start_date"),
            end_date: row.get("end_date"),
            versions,
            events,
        });
    }
    let payments=sqlx::query("SELECT id,project_id,amount,date,status FROM payments WHERE organization_id=$1 AND member_id=$2")
        .bind(organization_id).bind(member_id).fetch_all(db).await.map_err(db_error)?.into_iter().map(|r| DomainPayment{id:r.get("id"),project_id:r.get("project_id"),amount:r.get("amount"),date:r.get("date"),voided:r.get::<&str,_>("status")=="voided"}).collect();
    Ok((pledges, payments))
}
pub(crate) async fn replay_check(
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    key: Uuid,
    fingerprint: &str,
) -> Result<Option<Response>, Problem> {
    let inserted = sqlx::query("INSERT INTO idempotency(organization_id,user_id,key,fingerprint,status,response) VALUES($1,$2,$3,$4,0,'{}') ON CONFLICT DO NOTHING")
        .bind(p.organization_id).bind(p.id).bind(key).bind(fingerprint).execute(&mut **tx).await.map_err(db_error)?.rows_affected();
    if inserted == 1 {
        return Ok(None);
    }
    let row = sqlx::query("SELECT fingerprint,status,response,location,etag FROM idempotency WHERE organization_id=$1 AND user_id=$2 AND key=$3")
        .bind(p.organization_id).bind(p.id).bind(key).fetch_one(&mut **tx).await.map_err(db_error)?;
    if row.get::<String, _>("fingerprint") != fingerprint {
        return Err(Problem::new(
            StatusCode::CONFLICT,
            "idempotency_key_reused",
            "Idempotency key was used for a different request.",
        ));
    }
    let status =
        StatusCode::from_u16(row.get::<i32, _>("status") as u16).map_err(Problem::internal)?;
    let response: Value = row.get("response");
    let mut result = (status, Json(response)).into_response();
    if let Some(value) = row
        .get::<Option<String>, _>("location")
        .and_then(|s| s.parse().ok())
    {
        result
            .headers_mut()
            .insert(axum::http::header::LOCATION, value);
    }
    if let Some(value) = row
        .get::<Option<String>, _>("etag")
        .and_then(|s| s.parse().ok())
    {
        result.headers_mut().insert(axum::http::header::ETAG, value);
    }
    Ok(Some(no_store(result)))
}
pub(crate) fn db_error(error: sqlx::Error) -> Problem {
    if let sqlx::Error::Database(ref error) = error {
        if error.is_unique_violation() {
            return Problem::conflict();
        }
        if error.is_foreign_key_violation() {
            return Problem::field_validation(
                "/",
                "invalid_reference",
                "A referenced record does not exist or is no longer available.",
            );
        }
    }
    Problem::internal(error)
}
pub(crate) async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    action: &str,
    entity_type: &str,
    id: Uuid,
    reason: Option<&str>,
) -> Result<(), Problem> {
    sqlx::query("INSERT INTO audit(id,organization_id,actor_id,action,entity_type,entity_id,request_id,reason) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(Uuid::new_v4()).bind(p.organization_id).bind(p.id).bind(action).bind(entity_type).bind(id).bind(Uuid::new_v4()).bind(reason)
        .execute(&mut **tx).await.map_err(db_error)?;
    Ok(())
}
