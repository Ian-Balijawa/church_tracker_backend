use crate::http::contribution::Problem;
use crate::service::contribution::today;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListQuery {
    pub(crate) page: Option<u32>,
    pub(crate) page_size: Option<u32>,
    pub(crate) q: Option<String>,
    pub(crate) sort: Option<String>,
    pub(crate) as_of: Option<NaiveDate>,
}

impl ListQuery {
    pub(crate) fn validate(&self, sorts: &[&str]) -> Result<(i64, i64), Problem> {
        let page = self.page.unwrap_or(1);
        let page_size = self.page_size.unwrap_or(25);
        if page == 0 {
            return Err(Problem::field_validation(
                "page",
                "invalid_page",
                "Page must be at least 1.",
            ));
        }
        if !(1..=100).contains(&page_size) {
            return Err(Problem::field_validation(
                "pageSize",
                "invalid_page_size",
                "Page size must be between 1 and 100.",
            ));
        }
        if self
            .q
            .as_ref()
            .is_some_and(|q| q.trim().chars().count() > 100)
        {
            return Err(Problem::field_validation(
                "q",
                "too_long",
                "Search text cannot exceed 100 characters.",
            ));
        }
        if self
            .sort
            .as_deref()
            .is_some_and(|sort| !sorts.contains(&sort.trim_start_matches('-')))
        {
            return Err(Problem::field_validation(
                "sort",
                "invalid_sort",
                "Choose one of the supported sort fields.",
            ));
        }
        if self.as_of.is_some_and(|date| date > today()) {
            return Err(Problem::field_validation(
                "asOf",
                "invalid_date",
                "asOf cannot be in the future.",
            ));
        }
        Ok((
            i64::from(page_size),
            i64::from(page.saturating_sub(1)) * i64::from(page_size),
        ))
    }
    pub(crate) fn meta(&self, total: i64) -> Value {
        let page = self.page.unwrap_or(1);
        let size = self.page_size.unwrap_or(25);
        json!({"page":page,"pageSize":size,"total":total,"totalPages":if total == 0 {0} else {(total + i64::from(size) - 1) / i64::from(size)}})
    }
}
