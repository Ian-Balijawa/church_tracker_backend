use chrono::NaiveDate;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectCreate {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) owner_church_id: Uuid,
    pub(crate) target: i64,
    pub(crate) start_date: NaiveDate,
    pub(crate) end_date: Option<NaiveDate>,
}
