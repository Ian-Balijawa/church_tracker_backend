use chrono::{FixedOffset, NaiveDate, Utc};
use contributions_domain::MAX_MONEY;

pub(crate) fn valid_date(date: NaiveDate) -> bool {
    date <= today()
}
pub(crate) fn valid_money(value: i64, positive: bool) -> bool {
    ((if positive { 1 } else { 0 })..=MAX_MONEY).contains(&value)
}
pub(crate) fn today() -> NaiveDate {
    let kampala = FixedOffset::east_opt(3 * 3600).expect("valid offset");
    Utc::now().with_timezone(&kampala).date_naive()
}
pub(crate) fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.trim().chars().count() <= 200
}
pub(crate) fn valid_phone(value: &str) -> bool {
    let bytes = value.as_bytes();
    (8..=16).contains(&bytes.len()) && bytes[0] == b'+' && bytes[1..].iter().all(u8::is_ascii_digit)
}
