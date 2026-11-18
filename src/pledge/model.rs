use chrono::NaiveDate;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PledgeCreate {
    pub(crate) id: Uuid,
    pub(crate) member_id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) amount: i64,
    pub(crate) frequency: String,
    pub(crate) start_date: NaiveDate,
    pub(crate) end_date: Option<NaiveDate>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TransitionBody {
    pub(crate) effective_date: NaiveDate,
    pub(crate) reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VersionBody {
    pub(crate) id: Uuid,
    pub(crate) amount: i64,
    pub(crate) effective_from: NaiveDate,
    pub(crate) reason: Option<String>,
}
