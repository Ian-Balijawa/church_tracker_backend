use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MemberCreate {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) phone: String,
    pub(crate) home_church_id: Uuid,
}
