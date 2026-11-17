use chrono::NaiveDate;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PaymentCreate {
    pub(crate) id: Uuid,
    pub(crate) member_id: Uuid,
    pub(crate) project_id: Option<Uuid>,
    pub(crate) amount: i64,
    pub(crate) date: NaiveDate,
    pub(crate) method: String,
    pub(crate) note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoidBody {
    pub(crate) reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreviewBody {
    pub(crate) id: Uuid,
    pub(crate) member_id: Uuid,
    pub(crate) amount: i64,
    pub(crate) date: NaiveDate,
    pub(crate) project_id: Option<Uuid>,
    pub(crate) as_of: Option<NaiveDate>,
}
