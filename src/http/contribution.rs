use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Query, Request, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct Problem {
    status: StatusCode,
    code: &'static str,
    detail: &'static str,
}

impl Problem {
    pub(crate) fn new(status: StatusCode, code: &'static str, detail: &'static str) -> Self {
        Self {
            status,
            code,
            detail,
        }
    }
    pub(crate) fn validation() -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_error",
            "One or more fields are invalid.",
        )
    }
    pub(crate) fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Authentication required.",
        )
    }
    pub(crate) fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "Access denied.")
    }
    pub(crate) fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "Record not found.")
    }
    pub(crate) fn conflict() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "reference_conflict",
            "Operation conflicts with current state.",
        )
    }
    pub(crate) fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "contributions API database failure");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "The request could not be completed.",
        )
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(json!({
            "type": format!("https://api.example.org/problems/{}", self.code),
            "title": self.status.canonical_reason().unwrap_or("Error"),
            "status": self.status.as_u16(), "code": self.code,
            "detail": self.detail, "requestId": Uuid::new_v4(),
            "errors": if self.status == StatusCode::UNPROCESSABLE_ENTITY { Some(Vec::<Value>::new()) } else { None },
        }))).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("900"));
        }
        no_store(response)
    }
}

pub(crate) fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

pub(crate) fn item<T: Serialize>(
    value: T,
    status: StatusCode,
    etag: Option<String>,
    location: Option<String>,
) -> Response {
    let mut response = (status, Json(json!({"data": value}))).into_response();
    if let Some(etag) = etag.and_then(|v| HeaderValue::from_str(&v).ok()) {
        response.headers_mut().insert(header::ETAG, etag);
    }
    if let Some(location) = location.and_then(|v| HeaderValue::from_str(&v).ok()) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    no_store(response)
}

pub(crate) fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub(crate) fn etag(id: Uuid, version: i64) -> String {
    format!("\"{id}:{version}\"")
}

pub(crate) struct ContractJson<T>(pub T);
impl<S, T> FromRequest<S> for ContractJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Problem;
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|error| match error {
                JsonRejection::JsonDataError(_) => Problem::validation(),
                JsonRejection::MissingJsonContentType(_) => Problem::new(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "unsupported_media_type",
                    "Content-Type must be application/json.",
                ),
                _ => Problem::new(
                    StatusCode::BAD_REQUEST,
                    "malformed_json",
                    "Request JSON is malformed.",
                ),
            })
    }
}
pub(crate) struct ContractQuery<T>(pub T);
impl<S, T> FromRequestParts<S> for ContractQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Problem;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_| Problem::validation())
    }
}
pub(crate) fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, Problem> {
    headers
        .get(name)
        .ok_or_else(|| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "malformed_header",
                "A required header is missing.",
            )
        })?
        .to_str()
        .map_err(|_| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "malformed_header",
                "A header is malformed.",
            )
        })
}
pub(crate) fn if_match(headers: &HeaderMap, id: Uuid, version: i64) -> Result<(), Problem> {
    let actual = headers
        .get(header::IF_MATCH)
        .ok_or_else(|| {
            Problem::new(
                StatusCode::PRECONDITION_REQUIRED,
                "precondition_required",
                "If-Match is required.",
            )
        })?
        .to_str()
        .map_err(|_| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "malformed_header",
                "If-Match is malformed.",
            )
        })?;
    if actual != etag(id, version) {
        return Err(Problem::new(
            StatusCode::PRECONDITION_FAILED,
            "stale_version",
            "The resource has changed.",
        ));
    }
    Ok(())
}

pub(crate) fn req_key(headers: &HeaderMap) -> Result<Uuid, Problem> {
    header_str(headers, "idempotency-key")?
        .parse()
        .map_err(|_| Problem::validation())
}

pub(crate) fn list_response(data: Vec<Value>, meta: Value) -> Response {
    no_store((StatusCode::OK, Json(json!({"data":data,"meta":meta}))).into_response())
}
