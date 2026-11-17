use serde_json::{Map, Value, json};
use utoipa::openapi::OpenApi;

/// Published schema for endpoints currently implemented by this server.
pub fn document() -> OpenApi {
    let mut paths = Map::new();
    let endpoints: &[(&str, &[(&str, u16)])] = &[
        ("/auth/csrf", &[("get", 200)]),
        ("/auth/login", &[("post", 200)]),
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
                "/health" | "/ready" | "/auth/csrf" | "/auth/login"
            );
            let unsafe_method = *method != "get";
            let idempotent = unsafe_method
                && !relative.starts_with("/auth/")
                && *relative != "/payments/preview"
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
            if unsafe_method {
                parameters.push(json!({"name":"X-CSRF-Token","in":"header","required":true,"schema":{"type":"string"}}));
            }
            if idempotent {
                parameters.push(json!({"name":"Idempotency-Key","in":"header","required":true,"schema":{"type":"string","format":"uuid"}}));
            }
            if conditional {
                parameters.push(json!({"name":"If-Match","in":"header","required":true,"schema":{"type":"string"}}));
            }
            let mut responses = Map::new();
            responses.insert(success.to_string(), if *success == 204 { json!({"description":"No content"}) } else { json!({"description":"Success","content":{"application/json":{"schema":{"type":"object"}}}}) });
            for code in [400, 401, 403, 404, 409, 412, 415, 422, 428, 429, 500] {
                responses.insert(code.to_string(),json!({"description":"Problem detail","content":{"application/problem+json":{"schema":{"$ref":"#/components/schemas/Problem"}}}}));
            }
            let mut operation = json!({"operationId":format!("{}_{}",method,relative.trim_matches('/').replace(['/', '{', '}', '-'], "_")),"tags":[relative.split('/').nth(1).unwrap_or("health")],"responses":responses,"parameters":parameters});
            if !public {
                operation["security"] = json!([{"sessionCookie":[]}]);
            }
            if unsafe_method && *success != 204 && *relative != "/projects/{id}/members/{member_id}"
            {
                operation["requestBody"] = json!({"required":true,"content":{"application/json":{"schema":{"type":"object"}}}});
            }
            operations.insert((*method).to_owned(), operation);
        }
        paths.insert(path, Value::Object(operations));
    }
    serde_json::from_value(json!({
        "openapi":"3.1.0",
        "info":{"title":"Church Contributions API","version":"1.0.0","description":"Implemented v1 endpoints. The full proposed contract includes additional endpoints not yet available."},
        "paths":paths,
        "components":{"securitySchemes":{"sessionCookie":{"type":"apiKey","in":"cookie","name":"mc_session"}},"schemas":{"Problem":{"type":"object","required":["type","title","status","code","detail","requestId"],"properties":{"type":{"type":"string"},"title":{"type":"string"},"status":{"type":"integer"},"code":{"type":"string"},"detail":{"type":"string"},"requestId":{"type":"string","format":"uuid"}}}}}
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
