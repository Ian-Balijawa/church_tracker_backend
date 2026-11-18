use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ChurchCreate {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) phone: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ChurchPatch {
    pub(crate) name: Option<String>,
    pub(crate) phone: Option<Option<String>>,
}
