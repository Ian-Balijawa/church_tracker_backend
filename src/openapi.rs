use serde_json::{Map, Value, json};
use utoipa::openapi::OpenApi;

fn auth_request_schema(path: &str, method: &str) -> Option<&'static str> {
    match (path, method) {
        ("/auth/login", "post") => Some("PhonePasswordRequest"),
        ("/auth/register", "post") => Some("RegisterRequest"),
        ("/auth/phone/otp", "post") => Some("PhoneRequest"),
        ("/auth/phone/verify", "post") => Some("PhoneOtpRequest"),
        ("/auth/email/otp", "post") => Some("EmailRequest"),
        ("/auth/email/verify", "post") => Some("EmailOtpRequest"),
        ("/auth/unlock", "post") => Some("UnlockRequest"),
        ("/auth/pin", "put") => Some("SetPinRequest"),
        ("/auth/pin", "delete") => Some("CurrentPasswordRequest"),
        ("/auth/password", "put") => Some("ChangePasswordRequest"),
        ("/admin-users", "post") => Some("AdminUserRequest"),
        _ => None,
    }
}

/// Published schema for endpoints currently implemented by this server.
pub fn document() -> OpenApi {
    let mut paths = Map::new();
    let endpoints: &[(&str, &[(&str, u16)])] = &[
        ("/auth/csrf", &[("get", 200)]),
        ("/auth/login", &[("post", 200)]),
        ("/auth/register", &[("post", 201)]),
        ("/auth/phone/otp", &[("post", 204)]),
        ("/auth/phone/verify", &[("post", 204)]),
        ("/auth/email/otp", &[("post", 204)]),
        ("/auth/email/verify", &[("post", 204)]),
        ("/auth/logout", &[("post", 204)]),
        ("/auth/session", &[("get", 200)]),
        ("/auth/sessions", &[("get", 200)]),
        ("/auth/sessions/{id}", &[("delete", 204)]),
        ("/auth/unlock", &[("post", 204)]),
        ("/auth/pin", &[("put", 204), ("delete", 204)]),
        ("/auth/password", &[("put", 204)]),
        ("/admin-users", &[("post", 201)]),
        ("/churches", &[("get", 200), ("post", 201)]),
        ("/churches/{id}", &[("get", 200), ("patch", 200)]),
        ("/members", &[("get", 200), ("post", 201)]),
        ("/members/{id}", &[("get", 200)]),
        ("/projects", &[("get", 200), ("post", 201)]),
        ("/projects/{id}", &[("get", 200)]),
        ("/projects/{id}/members/{member_id}", &[("put", 204)]),
        ("/pledges", &[("get", 200), ("post", 201)]),
        ("/pledges/{id}", &[("get", 200)]),
        ("/pledges/{id}/events", &[("get", 200)]),
        ("/pledges/{id}/pause", &[("post", 200)]),
        ("/pledges/{id}/resume", &[("post", 200)]),
        ("/pledges/{id}/cancel", &[("post", 200)]),
        ("/pledges/{id}/versions", &[("post", 200)]),
        ("/payments", &[("get", 200), ("post", 201)]),
        ("/payments/{id}", &[("get", 200)]),
        ("/payments/{id}/void", &[("post", 200)]),
        ("/payments/preview", &[("post", 200)]),
        ("/payments/collections", &[("post", 201)]),
        ("/payments/collections/{id}", &[("get", 200)]),
        ("/payments/webhooks/marzpay", &[("post", 200)]),
        ("/health", &[("get", 200)]),
        ("/ready", &[("get", 200)]),
    ];
    for (relative, methods) in endpoints {
        let path = if *relative == "/health" || *relative == "/ready" {
            relative.to_string()
        } else {
            format!("/api/v1{relative}")
        };
        let mut operations = Map::new();
        for (method, success) in *methods {
            let public = matches!(
                *relative,
                "/health"
                    | "/ready"
                    | "/auth/csrf"
                    | "/auth/login"
                    | "/auth/register"
                    | "/auth/phone/otp"
                    | "/auth/phone/verify"
                    | "/payments/webhooks/marzpay"
            );
            let unsafe_method = *method != "get";
            let idempotent = unsafe_method
                && !relative.starts_with("/auth/")
                && *relative != "/payments/preview"
                && *relative != "/payments/webhooks/marzpay"
                && *relative != "/admin-users";
            let conditional = matches!(
                *relative,
                "/churches/{id}"
                    | "/projects/{id}/members/{member_id}"
                    | "/pledges/{id}/pause"
                    | "/pledges/{id}/resume"
                    | "/pledges/{id}/cancel"
                    | "/pledges/{id}/versions"
                    | "/payments/{id}/void"
            ) && unsafe_method;
            let mut parameters = Vec::new();
            for name in ["id", "member_id"] {
                if relative.contains(&format!("{{{name}}}")) {
                    parameters.push(json!({"name":name,"in":"path","required":true,"schema":{"type":"string","format":"uuid"}}));
                }
            }
            if unsafe_method && *relative != "/payments/webhooks/marzpay" {
                parameters.push(json!({"name":"X-CSRF-Token","in":"header","required":true,"schema":{"type":"string"}}));
                parameters.push(json!({"name":"Origin","in":"header","required":true,"schema":{"type":"string","format":"uri"}}));
            }
            if *relative == "/payments/webhooks/marzpay" {
                parameters.push(json!({"name":"X-MarzPay-Timestamp","in":"header","required":false,"schema":{"type":"string"},"description":"Required when MARZ_PAY_WEBHOOK_SECRET is configured."}));
                parameters.push(json!({"name":"X-MarzPay-Signature","in":"header","required":false,"schema":{"type":"string"},"description":"Required when MARZ_PAY_WEBHOOK_SECRET is configured."}));
            }
            if unsafe_method
                && matches!(
                    *relative,
                    "/auth/register" | "/auth/phone/otp" | "/auth/phone/verify"
                )
            {
                parameters.push(json!({"name":"mc_csrf_anon","in":"cookie","required":true,"schema":{"type":"string"},"description":"Cookie returned by GET /auth/csrf."}));
            }
            if idempotent {
                parameters.push(json!({"name":"Idempotency-Key","in":"header","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
            if conditional {
                parameters.push(json!({"name":"If-Match","in":"header","required":true,"schema":{"type":"string"}}));
            }
            let mut responses = Map::new();
            responses.insert(success.to_string(), if *success == 204 { json!({"description":"No content"}) } else { json!({"description":"Success","content":{"application/json":{"schema":{"type":"object"}}}}) });
            for code in [
                400, 401, 403, 404, 409, 412, 415, 422, 428, 429, 500, 502, 503,
            ] {
                responses.insert(code.to_string(),json!({"description":"Problem detail","content":{"application/problem+json":{"schema":{"$ref":"#/components/schemas/Problem"}}}}));
            }
            let mut operation = json!({"operationId":format!("{}_{}",method,relative.trim_matches('/').replace(['/', '{', '}', '-'], "_")),"tags":[relative.split('/').nth(1).unwrap_or("health")],"responses":responses,"parameters":parameters});
            if !public {
                operation["security"] = json!([{"sessionCookie":[]}]);
            }
            let body_schema = auth_request_schema(relative, method).or_else(|| {
                if *relative == "/payments/collections" && *method == "post" {
                    Some("CreateCollectionRequest")
                } else {
                    None
                }
            });
            let has_request_body = body_schema.is_some() || (*success != 204 && unsafe_method);
            if unsafe_method
                && has_request_body
                && *relative != "/projects/{id}/members/{member_id}"
            {
                operation["requestBody"] = match body_schema {
                    Some(schema) => {
                        json!({"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{schema}")}}}})
                    }
                    None => {
                        json!({"required":true,"content":{"application/json":{"schema":{"type":"object"}}}})
                    }
                };
            }
            operations.insert((*method).to_owned(), operation);
        }
        paths.insert(path, Value::Object(operations));
    }
    serde_json::from_value(json!({
        "openapi":"3.1.0",
        "info":{"title":"Church Contributions API","version":"1.0.0","description":"Implemented v1 endpoints. The full proposed contract includes additional endpoints not yet available."},
        "paths":paths,
        "components":{
            "securitySchemes":{"sessionCookie":{"type":"apiKey","in":"cookie","name":"mc_session"}},
            "schemas":{
                "Problem":{"type":"object","required":["type","title","status","code","detail","requestId"],"properties":{"type":{"type":"string"},"title":{"type":"string"},"status":{"type":"integer"},"code":{"type":"string"},"detail":{"type":"string"},"requestId":{"type":"string","format":"uuid"},"errors":{"type":"array","description":"Present on validation errors; each entry identifies the invalid field using a JSON Pointer.","items":{"type":"object","required":["field","code","message"],"properties":{"field":{"type":"string"},"code":{"type":"string"},"message":{"type":"string"}}}}}},
                "PhonePasswordRequest":{"type":"object","required":["phone","password"],"additionalProperties":false,"properties":{"phone":{"type":"string","description":"Ugandan mobile number; accepted formats are normalized to 2567…"},"password":{"type":"string"}}},
                "RegisterRequest":{"type":"object","required":["name","phone","password"],"additionalProperties":false,"properties":{"name":{"type":"string","minLength":1,"maxLength":200},"phone":{"type":"string","description":"Ugandan mobile number in any common format."},"password":{"type":"string","minLength":8,"maxLength":1024},"email":{"type":"string","format":"email","description":"Optional email address."}}},
                "PhoneRequest":{"type":"object","required":["phone"],"additionalProperties":false,"properties":{"phone":{"type":"string"}}},
                "PhoneOtpRequest":{"type":"object","required":["phone","otp"],"additionalProperties":false,"properties":{"phone":{"type":"string"},"otp":{"type":"string","pattern":"^[0-9]{5}$"}}},
                "EmailRequest":{"type":"object","required":["email"],"additionalProperties":false,"properties":{"email":{"type":"string","format":"email"}}},
                "EmailOtpRequest":{"type":"object","required":["email","otp"],"additionalProperties":false,"properties":{"email":{"type":"string","format":"email"},"otp":{"type":"string","pattern":"^[0-9]{5}$"}}},
                "UnlockRequest":{"type":"object","required":["method","credential"],"additionalProperties":false,"properties":{"method":{"type":"string","enum":["password","pin"]},"credential":{"type":"string"}}},
                "SetPinRequest":{"type":"object","required":["currentPassword","newPin"],"additionalProperties":false,"properties":{"currentPassword":{"type":"string"},"newPin":{"type":"string","pattern":"^[0-9]{4,12}$"}}},
                "CurrentPasswordRequest":{"type":"object","required":["currentPassword"],"additionalProperties":false,"properties":{"currentPassword":{"type":"string"}}},
                "ChangePasswordRequest":{"type":"object","required":["currentPassword","newPassword"],"additionalProperties":false,"properties":{"currentPassword":{"type":"string"},"newPassword":{"type":"string","minLength":8,"maxLength":1024}}},
                "AdminUserRequest":{"type":"object","required":["name","phone","password"],"additionalProperties":false,"properties":{"name":{"type":"string","minLength":1,"maxLength":200},"phone":{"type":"string"},"password":{"type":"string","minLength":8}}},
                "CreateCollectionRequest":{"type":"object","required":["memberId","amount","phone"],"additionalProperties":false,"properties":{"memberId":{"type":"string","format":"uuid"},"projectId":{"type":"string","format":"uuid"},"amount":{"type":"integer","minimum":500,"maximum":10000000,"description":"Whole UGX amount."},"phone":{"type":"string","description":"Ugandan MTN or Airtel number. Common local and international formats are accepted and normalized."},"description":{"type":"string","maxLength":255}}}
            }
        }
    })).expect("valid generated OpenAPI")
}

#[cfg(test)]
mod tests {
    #[test]
    fn schema_lists_church_tracker_routes_only() {
        let doc = serde_json::to_value(super::document()).unwrap();
        assert!(doc["paths"]["/api/v1/payments"]["post"].is_object());
        assert!(doc["paths"]["/api/v1/auth/login"]["post"].is_object());
        assert!(doc["paths"].get("/api/v1/tasks").is_none());
    }
}
