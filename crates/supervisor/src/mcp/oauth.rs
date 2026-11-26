use super::{store::*, tools::Grant};
use anyhow::{ensure, Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};
use url::Url;

pub fn safe_url(s: &str) -> bool {
    Url::parse(s).is_ok_and(|u| {
        u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
            && (u.scheme() == "https"
                || (u.scheme() == "http"
                    && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    })
}
pub fn issuer(state: &Supervisor) -> String {
    std::env::var("POCKYMOE_MCP_ISSUER")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{}", state.config.port))
        .trim_end_matches('/')
        .to_owned()
}
pub fn resource(state: &Supervisor) -> String {
    format!("{}/mcp", issuer(state))
}
fn error(e: anyhow::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"invalid_request","error_description":e.to_string()})),
    )
        .into_response()
}
fn response(r: Result<Value>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => error(e),
    }
}
pub async fn metadata(State(s): State<Arc<Supervisor>>) -> Response {
    let i = issuer(&s);
    response((|| {
        ensure!(safe_url(&i), "issuer must use HTTPS or loopback");
        Ok(
            json!({"issuer":i,"authorization_endpoint":format!("{i}/oauth/mcp/authorize"),"token_endpoint":format!("{i}/oauth/mcp/token"),"registration_endpoint":format!("{i}/oauth/mcp/register"),"revocation_endpoint":format!("{i}/oauth/mcp/revoke"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"scopes_supported":["read","supervised","full"]}),
        )
    })())
}
pub async fn protected(State(s): State<Arc<Supervisor>>) -> Json<Value> {
    Json(
        json!({"resource":resource(&s),"authorization_servers":[issuer(&s)],"scopes_supported":["read","supervised","full"],"bearer_methods_supported":["header"]}),
    )
}
pub async fn register(State(s): State<Arc<Supervisor>>, Json(v): Json<Value>) -> Response {
    response(transaction(&s, |db| {
        rate(db, &format!("register:{}", now() / 60), 30)?;
        let redirects = v["redirect_uris"]
            .as_array()
            .context("redirect_uris required")?;
        ensure!(
            !redirects.is_empty() && redirects.len() <= 8,
            "invalid redirect count"
        );
        ensure!(
            redirects.iter().all(|r| r.as_str().is_some_and(safe_url)),
            "redirects must use HTTPS or loopback without fragments"
        );
        ensure!(
            v["token_endpoint_auth_method"]
                .as_str()
                .is_none_or(|m| m == "none"),
            "public clients only"
        );
        let id = secret();
        let name = v["client_name"].as_str().unwrap_or("MCP client");
        ensure!(name.len() <= 200, "client name too long");
        let client = json!({"client_id":id,"client_name":name,"redirect_uris":redirects,"token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"response_types":["code"]});
        put(db, "client", &id, &client, 30 * 86400)?;
        Ok(client)
    }))
}
pub async fn authorize(
    State(s): State<Arc<Supervisor>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let result = transaction(&s, |db| {
        rate(db, &format!("authorize:{}", now() / 60), 60)?;
        let param = |k: &str| {
            q.get(k)
                .map(String::as_str)
                .context("missing authorization parameter")
        };
        ensure!(safe_url(&issuer(&s)), "issuer must use HTTPS or loopback");
        let client = get(db, "client", param("client_id")?)?;
        let redirect = param("redirect_uri")?;
        ensure!(
            client["redirect_uris"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == redirect),
            "redirect mismatch"
        );
        ensure!(
            param("response_type")? == "code" && param("code_challenge_method")? == "S256",
            "code + S256 required"
        );
        let challenge = param("code_challenge")?;
        ensure!(
            challenge.len() == 43
                && challenge
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid challenge"
        );
        ensure!(param("resource")? == resource(&s), "resource mismatch");
        let requested = q.get("scope").map(String::as_str).unwrap_or("supervised");
        let mut scope = "read";
        for value in requested.split_whitespace() {
            ensure!(
                matches!(value, "read" | "supervised" | "full"),
                "invalid scope"
            );
            if value == "full" || (value == "supervised" && scope != "full") {
                scope = value;
            }
        }
        ensure!(!requested.trim().is_empty(), "scope required");
        let ticket = secret();
        put(
            db,
            "consent",
            &ticket,
            &json!({"query":q,"client":client,"requestedAccess":scope}),
            600,
        )?;
        Ok(ticket)
    });
    match result {
        Ok(ticket) => Redirect::to(&format!("/?mcpConsent={ticket}")).into_response(),
        Err(e) => error(e),
    }
}
pub async fn consent_get(State(s): State<Arc<Supervisor>>, Path(id): Path<String>) -> Response {
    response(transaction(&s, |db| get(db, "consent", &id)))
}
pub async fn consent_post(
    State(s): State<Arc<Supervisor>>,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> Response {
    response(transaction(&s, |db| {
        let consent = get(db, "consent", &id)?;
        let q = &consent["query"];
        let mut redirect = Url::parse(q["redirect_uri"].as_str().unwrap())?;
        {
            let mut pairs = redirect.query_pairs_mut();
            if let Some(state) = q["state"].as_str() {
                pairs.append_pair("state", state);
            }
            if v["approve"] != true {
                pairs.append_pair("error", "access_denied");
            } else {
                let access = v["access"].as_str().context("access required")?;
                let rank = |s: &str| match s {
                    "read" => 0,
                    "supervised" => 1,
                    "full" => 2,
                    _ => 99,
                };
                ensure!(
                    rank(access) <= rank(consent["requestedAccess"].as_str().unwrap()),
                    "grant cannot widen requested scope"
                );
                let workspaces = if v["workspaces"].is_null() {
                    Value::Null
                } else {
                    let ws = v["workspaces"]
                        .as_array()
                        .context("workspaces must be array")?;
                    for w in ws {
                        s.get_workspace(w.as_str().context("invalid workspace")?)?;
                    }
                    v["workspaces"].clone()
                };
                let grant = secret();
                put(
                    db,
                    "grant",
                    &grant,
                    &json!({"id":grant,"clientId":q["client_id"],"clientName":consent["client"]["client_name"],"access":access,"workspaces":workspaces,"createdAt":now(),"lastUsedAt":now()}),
                    30 * 86400,
                )?;
                let code = secret();
                put(
                    db,
                    "code",
                    &hash(&code),
                    &json!({"grant":grant,"clientId":q["client_id"],"redirect":q["redirect_uri"],"challenge":q["code_challenge"],"resource":q["resource"]}),
                    120,
                )?;
                pairs.append_pair("code", &code);
            }
        }
        delete(db, "consent", &id)?;
        Ok(json!({"redirect":redirect.as_str()}))
    }))
}
fn mint(db: &rusqlite::Connection, grant: &str, client: &str, resource: &str) -> Result<Value> {
    let g = get(db, "grant", grant)?;
    let access = secret();
    let refresh = secret();
    let record = json!({"grant":grant,"clientId":client,"resource":resource,"expiresAt":now()+900});
    put(db, "access", &hash(&access), &record, 900)?;
    put(
        db,
        "refresh",
        &hash(&refresh),
        &json!({"grant":grant,"clientId":client,"resource":resource,"expiresAt":now()+30*86400,"used":false}),
        60 * 86400,
    )?;
    Ok(
        json!({"access_token":access,"refresh_token":refresh,"token_type":"Bearer","expires_in":900,"scope":g["access"]}),
    )
}
pub async fn token(
    State(s): State<Arc<Supervisor>>,
    Form(q): Form<HashMap<String, String>>,
) -> Response {
    // Commit invalid refresh reuse revocation, even though the exchange fails.
    let result = transaction(&s, |db| {
        rate(db, &format!("token:{}", now() / 60), 120)?;
        let param = |k: &str| {
            q.get(k)
                .map(String::as_str)
                .context("missing token parameter")
        };
        let resource_param = param("resource")?;
        ensure!(resource_param == resource(&s), "resource mismatch");
        let client = param("client_id")?;
        match param("grant_type")? {
            "authorization_code" => {
                let code = param("code")?;
                let record = get(db, "code", &hash(code))?;
                ensure!(
                    record["clientId"] == client
                        && record["resource"] == resource_param
                        && record["redirect"] == param("redirect_uri")?,
                    "code binding mismatch"
                );
                let verifier = param("code_verifier")?;
                ensure!(
                    (43..=128).contains(&verifier.len())
                        && verifier
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)),
                    "invalid verifier"
                );
                ensure!(
                    record["challenge"]
                        == URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
                    "PKCE verification failed"
                );
                delete(db, "code", &hash(code))?;
                Ok(Ok(mint(
                    db,
                    record["grant"].as_str().unwrap(),
                    client,
                    resource_param,
                )?))
            }
            "refresh_token" => {
                let refresh = param("refresh_token")?;
                let mut record = get(db, "refresh", &hash(refresh))?;
                ensure!(
                    record["clientId"] == client && record["resource"] == resource_param,
                    "refresh binding mismatch"
                );
                let grant = record["grant"].as_str().unwrap().to_owned();
                if record["used"] == true {
                    delete(db, "grant", &grant)?;
                    return Ok(Err(anyhow::anyhow!("refresh token reuse; grant revoked")));
                }
                ensure!(
                    record["expiresAt"].as_i64().unwrap_or(0) > now(),
                    "refresh expired"
                );
                record["used"] = json!(true);
                put(db, "refresh", &hash(refresh), &record, 60 * 86400)?;
                Ok(Ok(mint(db, &grant, client, resource_param)?))
            }
            _ => anyhow::bail!("unsupported grant type"),
        }
    });
    let mut r = response(result.and_then(|v| v));
    r.headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    r
}
pub async fn revoke(
    State(s): State<Arc<Supervisor>>,
    Form(q): Form<HashMap<String, String>>,
) -> Response {
    response(transaction(&s, |db| {
        rate(db, &format!("revoke:{}", now() / 60), 120)?;
        if let (Some(token), Some(client)) = (q.get("token"), q.get("client_id")) {
            for kind in ["access", "refresh"] {
                if let Ok(r) = get(db, kind, &hash(token)) {
                    if r["clientId"] == client.as_str() {
                        delete(db, "grant", r["grant"].as_str().unwrap())?;
                    }
                }
            }
        }
        Ok(json!({}))
    }))
}
pub fn authenticate(s: &Supervisor, h: &HeaderMap) -> Result<(String, Grant)> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .context("MCP bearer token required")?;
    transaction(s, |db| {
        let a = get(db, "access", &hash(token))?;
        ensure!(
            a["expiresAt"].as_i64().unwrap_or(0) > now() && a["resource"] == resource(s),
            "expired or wrong audience"
        );
        let mut g = get(db, "grant", a["grant"].as_str().unwrap())?;
        g["lastUsedAt"] = json!(now());
        let remaining = (g["createdAt"].as_i64().unwrap_or(0) + 30 * 86400 - now()).max(0);
        ensure!(remaining > 0, "grant expired");
        put(db, "grant", a["grant"].as_str().unwrap(), &g, remaining)?;
        Ok((
            a["grant"].as_str().unwrap().to_owned(),
            Grant {
                device: None,
                client: g["clientId"].as_str().unwrap().into(),
                access: g["access"].as_str().unwrap().into(),
                workspaces: serde_json::from_value(g["workspaces"].clone())?,
            },
        ))
    })
}
pub async fn settings(State(s): State<Arc<Supervisor>>) -> Response {
    response(transaction(&s, |db| {
        Ok(json!({"endpoint":resource(&s),"clients":list(db,"grant")?,"calls":list(db,"audit")?}))
    }))
}
pub async fn remove(State(s): State<Arc<Supervisor>>, Path(id): Path<String>) -> Response {
    response(transaction(&s, |db| {
        delete(db, "grant", &id)?;
        Ok(json!({"revoked":true}))
    }))
}
