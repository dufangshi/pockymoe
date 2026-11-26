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
/// An OAuth error code (RFC 6749 §5.2, RFC 7591 §3.2.2, RFC 8707 §2).
#[derive(Debug)]
pub struct OAuthError(pub &'static str, pub String);
impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}
impl std::error::Error for OAuthError {}
fn fail(code: &'static str, message: &str) -> anyhow::Error {
    OAuthError(code, message.into()).into()
}
fn code_of(e: &anyhow::Error) -> &'static str {
    e.downcast_ref::<OAuthError>()
        .map_or("invalid_request", |e| e.0)
}
fn error(e: anyhow::Error) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":code_of(&e),"error_description":e.to_string()})),
    )
        .into_response()
}
/// Counts every attempt, failed ones included. The limit runs in its own
/// transaction because a failed request rolls back its own.
fn limited(s: &Supervisor, endpoint: &str, limit: u64) -> Option<Response> {
    let bucket = format!("{endpoint}:{}", now() / 60);
    if transaction(s, |db| rate(db, &bucket, limit)).is_ok() {
        return None;
    }
    let mut r = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"error":"temporarily_unavailable","error_description":"rate limit exceeded; retry in a minute"})),
    )
        .into_response();
    r.headers_mut().insert(
        "retry-after",
        (60 - now() % 60).to_string().parse().unwrap(),
    );
    Some(r)
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
    if let Some(r) = limited(&s, "register", 30) {
        return r;
    }
    let mut r = response(transaction(&s, |db| {
        let redirects = v["redirect_uris"]
            .as_array()
            .ok_or_else(|| fail("invalid_redirect_uri", "redirect_uris required"))?;
        if redirects.is_empty() || redirects.len() > 8 {
            return Err(fail("invalid_redirect_uri", "invalid redirect count"));
        }
        if !redirects.iter().all(|r| r.as_str().is_some_and(safe_url)) {
            return Err(fail(
                "invalid_redirect_uri",
                "redirects must use HTTPS or loopback without fragments",
            ));
        }
        if v["token_endpoint_auth_method"]
            .as_str()
            .is_some_and(|m| m != "none")
        {
            return Err(fail("invalid_client_metadata", "public clients only"));
        }
        let id = secret();
        let name = v["client_name"].as_str().unwrap_or("MCP client");
        if name.len() > 200 {
            return Err(fail("invalid_client_metadata", "client name too long"));
        }
        let client = json!({"client_id":id,"client_name":name,"redirect_uris":redirects,"token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"response_types":["code"]});
        put(db, "client", &id, &client, 30 * 86400)?;
        Ok(client)
    }));
    if r.status() == StatusCode::OK {
        *r.status_mut() = StatusCode::CREATED;
    }
    r
}
pub async fn authorize(
    State(s): State<Arc<Supervisor>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = limited(&s, "authorize", 60) {
        return r;
    }
    // Until the client and its redirect are known, errors stay on this page
    // (RFC 6749 §4.1.2.1); after that they go back to the client.
    let client = transaction(&s, |db| {
        ensure!(safe_url(&issuer(&s)), "issuer must use HTTPS or loopback");
        let id = q.get("client_id").context("client_id required")?;
        let client = get(db, "client", id).map_err(|_| fail("invalid_client", "unknown client"))?;
        let redirect = q.get("redirect_uri").context("redirect_uri required")?;
        ensure!(
            client["redirect_uris"]
                .as_array()
                .is_some_and(|r| r.iter().any(|v| v == redirect.as_str())),
            "redirect mismatch"
        );
        Ok(client)
    });
    let client = match client {
        Ok(c) => c,
        Err(e) => return error(e),
    };
    let result = transaction(&s, |db| {
        let param = |k: &str| {
            q.get(k)
                .map(String::as_str)
                .ok_or_else(|| fail("invalid_request", &format!("{k} required")))
        };
        if param("response_type")? != "code" {
            return Err(fail("unsupported_response_type", "code required"));
        }
        if param("code_challenge_method")? != "S256" {
            return Err(fail("invalid_request", "PKCE S256 required"));
        }
        let challenge = param("code_challenge")?;
        ensure!(
            challenge.len() == 43
                && challenge
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid challenge"
        );
        // RFC 8707 makes the indicator optional; there is one resource here.
        if q.get("resource").is_some_and(|r| *r != resource(&s)) {
            return Err(fail("invalid_target", "resource mismatch"));
        }
        // Unknown scope values are ignored: they never widen access.
        let requested = q.get("scope").map(String::as_str).unwrap_or("");
        let rank = |v: &str| ["read", "supervised", "full"].iter().position(|l| *l == v);
        let scope = requested
            .split_whitespace()
            .filter_map(rank)
            .max()
            .map_or("supervised", |i| ["read", "supervised", "full"][i]);
        let mut query = q.clone();
        query.insert("resource".into(), resource(&s));
        let ticket = secret();
        put(
            db,
            "consent",
            &ticket,
            &json!({"query":query,"client":client,"requestedAccess":scope}),
            600,
        )?;
        Ok(ticket)
    });
    match result {
        Ok(ticket) => Redirect::to(&format!("/?mcpConsent={ticket}")).into_response(),
        Err(e) => {
            let mut back = Url::parse(&q["redirect_uri"]).expect("registered redirect");
            {
                let mut pairs = back.query_pairs_mut();
                pairs.append_pair("error", code_of(&e));
                pairs.append_pair("error_description", &e.to_string());
                if let Some(state) = q.get("state") {
                    pairs.append_pair("state", state);
                }
            }
            Redirect::to(back.as_str()).into_response()
        }
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
    if let Some(r) = limited(&s, "token", 120) {
        return r;
    }
    // Commit invalid refresh reuse revocation, even though the exchange fails.
    let result = transaction(&s, |db| {
        let param = |k: &str| {
            q.get(k)
                .map(String::as_str)
                .ok_or_else(|| fail("invalid_request", &format!("{k} required")))
        };
        let resource_param = q.get("resource").cloned().unwrap_or_else(|| resource(&s));
        if resource_param != resource(&s) {
            return Err(fail("invalid_target", "resource mismatch"));
        }
        let client = param("client_id")?;
        let grant_error = |m: &str| fail("invalid_grant", m);
        match param("grant_type")? {
            "authorization_code" => {
                let code = param("code")?;
                let mut record = get(db, "code", &hash(code))
                    .map_err(|_| grant_error("authorization code not found or expired"))?;
                if record["used"] == true {
                    // RFC 6749 §4.1.2: a replayed code revokes what it issued.
                    delete(db, "grant", record["grant"].as_str().unwrap_or(""))?;
                    delete(db, "code", &hash(code))?;
                    return Ok(Err(grant_error("authorization code reuse; grant revoked")));
                }
                if record["clientId"] != client
                    || record["resource"] != resource_param.as_str()
                    || record["redirect"] != param("redirect_uri")?
                {
                    return Err(grant_error("code binding mismatch"));
                }
                let verifier = param("code_verifier")?;
                if !(43..=128).contains(&verifier.len())
                    || !verifier
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                {
                    return Err(grant_error("invalid verifier"));
                }
                if record["challenge"]
                    != URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
                {
                    return Err(grant_error("PKCE verification failed"));
                }
                record["used"] = json!(true);
                put(db, "code", &hash(code), &record, 120)?;
                Ok(Ok(mint(
                    db,
                    record["grant"].as_str().unwrap(),
                    client,
                    &resource_param,
                )?))
            }
            "refresh_token" => {
                let refresh = param("refresh_token")?;
                let mut record = get(db, "refresh", &hash(refresh))
                    .map_err(|_| grant_error("refresh token not found or expired"))?;
                if record["clientId"] != client || record["resource"] != resource_param.as_str() {
                    return Err(grant_error("refresh binding mismatch"));
                }
                let grant = record["grant"].as_str().unwrap().to_owned();
                if record["used"] == true {
                    delete(db, "grant", &grant)?;
                    return Ok(Err(grant_error("refresh token reuse; grant revoked")));
                }
                if record["expiresAt"].as_i64().unwrap_or(0) <= now() {
                    return Err(grant_error("refresh expired"));
                }
                if get(db, "grant", &grant).is_err() {
                    return Err(grant_error("grant revoked or expired"));
                }
                record["used"] = json!(true);
                put(db, "refresh", &hash(refresh), &record, 60 * 86400)?;
                Ok(Ok(mint(db, &grant, client, &resource_param)?))
            }
            _ => Err(fail("unsupported_grant_type", "unsupported grant type")),
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
    if let Some(r) = limited(&s, "revoke", 120) {
        return r;
    }
    response(transaction(&s, |db| {
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
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim())
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
