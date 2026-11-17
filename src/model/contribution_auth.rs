use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminUserBody {
    pub(crate) name: String,
    pub(crate) phone: String,
    pub(crate) password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LoginBody {
    pub(crate) phone: String,
    pub(crate) password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetPinBody {
    pub(crate) current_password: String,
    pub(crate) new_pin: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PasswordBody {
    pub(crate) current_password: String,
    pub(crate) new_password: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnlockBody {
    pub(crate) method: String,
    pub(crate) credential: String,
}
