//! The API contract (#312): every `/api` route with its method, a summary
//! and the token scope it needs, served as OpenAPI 3.1 at
//! `/api/openapi.json` (and `/api/v1/openapi.json`). A test checks the
//! table against the routes in the source, both ways.
//!
//! `/api/v1/...` is the versioned prefix; `/api/...` stays as an alias.

use axum::Json;
use serde_json::{Map, Value, json};

/// `(method, path, scope, summary)` of every API route.
pub(crate) const ROUTES: &[(&str, &str, &str, &str)] = &[
    (
        "GET",
        "/api/projects",
        "read",
        "List projects and their capture keys.",
    ),
    (
        "POST",
        "/api/projects",
        "admin",
        "Create a project; returns its key and snippet.",
    ),
    (
        "PATCH",
        "/api/projects/{project}",
        "admin",
        "Rename a project, set its time zone or its environment host rules.",
    ),
    (
        "GET",
        "/api/projects/{project}/environments",
        "read",
        "Hosts per environment, host rules and unclassified hosts.",
    ),
    (
        "GET",
        "/api/projects/{project}/vitals",
        "read",
        "Web Vitals at p75 with thresholds, worst pages, likely cause and ranked actions.",
    ),
    (
        "DELETE",
        "/api/projects/{project}",
        "admin",
        "Delete a project and all its data (`?confirm=`).",
    ),
    (
        "POST",
        "/api/projects/{project}/sql",
        "read",
        "Run read-only SQL on `events`.",
    ),
    (
        "GET",
        "/api/projects/{project}/board",
        "read",
        "The Focus board as JSON.",
    ),
    (
        "GET",
        "/api/projects/{project}/errors",
        "read",
        "Error groups.",
    ),
    (
        "GET",
        "/api/projects/{project}/breakdown",
        "read",
        "Users, sessions and events by one dimension.",
    ),
    (
        "GET",
        "/api/projects/{project}/people/{distinct_id}",
        "read",
        "One person's profile and recent events.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/people/{who}/forget",
        "admin",
        "Erase a person everywhere.",
    ),
    (
        "GET",
        "/api/projects/{project}/feature-map",
        "read",
        "The feature map in force.",
    ),
    (
        "PUT",
        "/api/projects/{project}/feature-map",
        "write",
        "Save a new feature map version.",
    ),
    (
        "GET",
        "/api/projects/{project}/feature-map/health",
        "read",
        "Map health: score and defects.",
    ),
    (
        "GET",
        "/api/projects/{project}/feature-map/versions",
        "read",
        "Saved versions of the map.",
    ),
    (
        "POST",
        "/api/feature-map/validate",
        "read",
        "Check a feature map without saving it.",
    ),
    (
        "GET",
        "/api/projects/{project}/tickets",
        "read",
        "Issues opened from findings.",
    ),
    (
        "POST",
        "/api/projects/{project}/tickets",
        "write",
        "Open an issue from a finding.",
    ),
    (
        "POST",
        "/api/projects/{project}/import",
        "write",
        "Import past events (JSON lines).",
    ),
    (
        "GET",
        "/api/projects/{project}/export",
        "read",
        "Export one day as JSON lines (`?date=`).",
    ),
    (
        "GET",
        "/api/projects/{project}/flows",
        "read",
        "Feature flows.",
    ),
    (
        "GET",
        "/api/projects/{project}/status",
        "read",
        "Uptime checks and incidents.",
    ),
    (
        "GET",
        "/api/projects/{project}/churn",
        "read",
        "Churn report and people at risk.",
    ),
    (
        "GET",
        "/api/projects/{project}/goals",
        "read",
        "Goals and their progress.",
    ),
    (
        "POST",
        "/api/projects/{project}/goals",
        "write",
        "Set or remove a goal.",
    ),
    (
        "GET",
        "/api/projects/{project}/timeline",
        "read",
        "Releases, map changes and notes.",
    ),
    (
        "POST",
        "/api/projects/{project}/timeline",
        "write",
        "Add a timeline note.",
    ),
    (
        "GET",
        "/api/projects/{project}/flags",
        "read",
        "Feature flags.",
    ),
    (
        "PUT",
        "/api/projects/{project}/flags/{key}",
        "write",
        "Create or change a flag.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/flags/{key}",
        "write",
        "Delete a flag.",
    ),
    (
        "GET",
        "/api/projects/{project}/surveys",
        "read",
        "Micro-surveys.",
    ),
    (
        "PUT",
        "/api/projects/{project}/surveys/{key}",
        "write",
        "Create or change a survey.",
    ),
    (
        "DELETE",
        "/api/projects/{project}/surveys/{key}",
        "write",
        "Delete a survey.",
    ),
    (
        "GET",
        "/api/projects/{project}/digest",
        "read",
        "The weekly digest.",
    ),
    (
        "GET",
        "/api/projects/{project}/replays",
        "read",
        "Recordings.",
    ),
    (
        "GET",
        "/api/projects/{project}/replays/{session}",
        "read",
        "One recording's chunks.",
    ),
    ("GET", "/api/audit", "admin", "The audit log."),
    (
        "GET",
        "/api/tokens",
        "admin",
        "API tokens (never their values).",
    ),
    (
        "POST",
        "/api/tokens",
        "admin",
        "Create a scoped API token; its value is shown once.",
    ),
    (
        "DELETE",
        "/api/tokens/{id}",
        "admin",
        "Revoke an API token.",
    ),
    (
        "POST",
        "/api/login-links",
        "admin",
        "A one-time login link or an invite.",
    ),
    ("GET", "/api/users", "admin", "Dashboard users."),
    (
        "PATCH",
        "/api/users/{id}",
        "admin",
        "Change a user's role, projects or zone.",
    ),
    ("DELETE", "/api/users/{id}", "admin", "Remove a user."),
    ("GET", "/api/openapi.json", "none", "This document."),
    (
        "GET",
        "/api/projects/{project}/feature-map/features/{key}",
        "read",
        "One feature: its context and numbers.",
    ),
    (
        "GET",
        "/api/projects/{project}/replays/{session}/comments",
        "read",
        "Comments on a recording.",
    ),
    (
        "POST",
        "/api/projects/{project}/replays/{session}/comments",
        "write",
        "Comment on a recording.",
    ),
    (
        "PUT",
        "/api/projects/{project}/sourcemaps/{release}/{bundle}",
        "write",
        "Upload a source map for a release.",
    ),
];

fn params(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|name| json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}}))
        .collect()
}

/// The OpenAPI 3.1 document of [`ROUTES`], relative to `/api/v1`.
pub(crate) fn document() -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for (method, path, scope, summary) in ROUTES {
        let rel = path.strip_prefix("/api").unwrap_or(path);
        let mut op = json!({
            "summary": summary,
            "x-scope": scope,
            "parameters": params(path),
            "responses": {
                "200": {"description": "OK", "content": {"application/json": {}}},
                "default": {"description": "An error", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}},
            },
        });
        if matches!(*method, "POST" | "PUT" | "PATCH") {
            op["requestBody"] =
                json!({"content": {"application/json": {"schema": {"type": "object"}}}});
        }
        if *scope != "none" {
            op["security"] = json!([{"bearer": []}]);
        }
        let entry = paths.entry(rel.to_owned()).or_insert_with(|| json!({}));
        entry[method.to_ascii_lowercase()] = op;
    }
    json!({
        "openapi": "3.1.0",
        "info": {"title": "OpenLens API", "version": env!("CARGO_PKG_VERSION")},
        "servers": [{"url": "/api/v1"}, {"url": "/api", "description": "Unversioned alias"}],
        "paths": paths,
        "components": {
            "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"}},
            "schemas": {"Error": {
                "type": "object",
                "required": ["code", "message"],
                "properties": {
                    "code": {"type": "string", "enum": crate::apierr::CODES},
                    "message": {"type": "string"},
                    "error": {"type": "string", "deprecated": true},
                },
            }},
        },
    })
}

/// `GET /api/openapi.json`.
pub(crate) async fn serve() -> Json<Value> {
    Json(document())
}
