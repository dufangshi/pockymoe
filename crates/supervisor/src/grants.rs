//! Owner-signed device ACL. All authority and lineage come from the local database.
use crate::owner_trust::{self, BrowserPrincipal, PinnedAccount};
use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    response::Response,
    Extension, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use pockymoe_protocol::access::{self, EffectiveAccess};
use pockymoe_runtime::{db::Database, Supervisor};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelayRef {
    pub kind: String,
    pub id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Grantee {
    pub user_id: String,
    pub label: String,
    pub account_key_id: String,
    pub account_public_key: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Grant {
    pub version: u32,
    pub grant_id: String,
    pub relay_ref: RelayRef,
    pub grantee: Grantee,
    pub scope: String,
    pub thread_id: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_scope: String,
    pub workspace_ids: Vec<String>,
    pub thread_access: String,
    pub workspace_access: String,
    pub can_create_threads: bool,
    pub expires_at: Option<i64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredGrant {
    #[serde(flatten)]
    grant: Grant,
    created_at: i64,
    created_by: String,
}
fn rows(c: &Connection) -> Result<Vec<StoredGrant>> {
    let rows = c.prepare("SELECT grant_json,created_at,created_by FROM device_grants ORDER BY created_at DESC,grant_id")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(s, created_at, created_by)| {
            Ok(StoredGrant {
                grant: serde_json::from_str(&s)?,
                created_at,
                created_by,
            })
        })
        .collect()
}
fn live_rows(c: &Connection, time: i64) -> Result<Vec<StoredGrant>> {
    Ok(rows(c)?
        .into_iter()
        .filter(|r| {
            supported_workspace_scope(&r.grant)
                && r.grant.expires_at.is_none_or(|expires| expires > time)
        })
        .collect())
}
pub(crate) fn live_account(
    c: &Connection,
    id: &str,
    time: i64,
) -> Result<Option<(PinnedAccount, String)>> {
    for row in live_rows(c, time)? {
        let g = row.grant.grantee;
        if g.account_key_id == id {
            return Ok(Some((
                PinnedAccount {
                    account_key_id: g.account_key_id,
                    account_public_key: g.account_public_key,
                },
                g.user_id,
            )));
        }
    }
    Ok(None)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.contains(['\n', '\r', '/', '?', '#', '%', '\\'])
}
fn supported_workspace_scope(g: &Grant) -> bool {
    // Device collections, search, and thread creation do not yet support a
    // selected-workspace projection. Reject this combination rather than widen it.
    g.scope != "device" || g.workspace_scope == "all"
}
fn validate(c: &Connection, id: &str, g: &Grant) -> Result<()> {
    let good = g.version == 1
        && id == g.grant_id
        && B64
            .decode(id)
            .is_ok_and(|b| b.len() == 16 && B64.encode(b) == id)
        && matches!(g.relay_ref.kind.as_str(), "share" | "grant")
        && valid_id(&g.relay_ref.id)
        && valid_id(&g.grantee.user_id)
        && g.grantee.label.len() <= 1024
        && matches!(g.scope.as_str(), "thread" | "workspace" | "device")
        && matches!(g.workspace_scope.as_str(), "all" | "selected")
        && supported_workspace_scope(g)
        && matches!(g.thread_access.as_str(), "read" | "control")
        && matches!(g.workspace_access.as_str(), "none" | "read" | "write")
        && g.thread_id.as_deref().is_none_or(valid_id)
        && g.workspace_id.as_deref().is_none_or(valid_id)
        && g.workspace_ids.len() <= 256
        && g.workspace_ids.iter().all(|id| valid_id(id))
        && (g.scope != "thread" || g.thread_id.is_some())
        && (g.scope != "workspace" || g.workspace_id.is_some())
        && (g.relay_ref.kind != "share" || g.scope == "thread");
    if !good {
        return owner_trust::reject(400, "invalid_grant");
    }
    if owner_trust::account_key_id(&g.grantee.account_public_key)
        .ok()
        .as_deref()
        != Some(&g.grantee.account_key_id)
    {
        return owner_trust::reject(400, "invalid_grant");
    }
    let own: Option<String> = c.query_row(
        "SELECT account_key_id FROM device_owner_trust WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if own.as_deref() == Some(&g.grantee.account_key_id) {
        return owner_trust::reject(400, "grantee_is_owner");
    }
    for row in rows(c)? {
        if row.grant.grant_id != id
            && row.grant.grantee.account_key_id == g.grantee.account_key_id
            && (row.grant.grantee.user_id != g.grantee.user_id
                || row.grant.grantee.account_public_key != g.grantee.account_public_key)
        {
            return owner_trust::reject(400, "invalid_grant");
        }
    }
    Ok(())
}
fn revoke_orphans(c: &Connection, time: i64) -> Result<()> {
    let registrations = c
        .prepare(
            "SELECT browser_id,account_key_id,user_id FROM browser_registrations WHERE role='grantee'",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?,r.get::<_,Option<String>>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, account, user) in registrations {
        if live_account(c, &account, time)?.is_none_or(|(_, live_user)| Some(live_user) != user) {
            c.execute(
                "DELETE FROM browser_registrations WHERE browser_id=?1",
                [id],
            )?;
        }
    }
    Ok(())
}
fn put(
    db: &Database,
    id: &str,
    grant: &Grant,
    principal: &BrowserPrincipal,
    time: i64,
) -> Result<Value> {
    if !principal.is_owner() {
        return owner_trust::reject(403, "owner_required");
    }
    db.with(|c| {
        let tx = Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        validate(&tx,id,grant)?;
        let created: Option<i64> = tx.query_row("SELECT created_at FROM device_grants WHERE grant_id=?1",[id],|r|r.get(0)).optional()?;
        if created.is_none() && tx.query_row("SELECT count(*) FROM device_grants",[],|r|r.get::<_,u64>(0))? >= 256 { return owner_trust::reject(409,"grant_limit"); }
        tx.execute("INSERT INTO device_grants(grant_id,grant_json,created_at,created_by) VALUES(?1,?2,?3,?4) ON CONFLICT(grant_id) DO UPDATE SET grant_json=excluded.grant_json",params![id,serde_json::to_string(grant)?,created.unwrap_or(time),principal.actor()])?;
        revoke_orphans(&tx,time)?;
        let stored = rows(&tx)?.into_iter().find(|r| r.grant.grant_id == id).context("grant missing")?;
        tx.commit()?;
        Ok(json!({"grant": stored}))
    })
}
fn delete(db: &Database, id: &str, time: i64) -> Result<Value> {
    db.with(|c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM device_grants WHERE grant_id=?1", [id])?;
        revoke_orphans(&tx, time)?;
        tx.commit()?;
        Ok(json!({}))
    })
}
fn row_access(g: &Grant) -> EffectiveAccess {
    EffectiveAccess {
        kind: "shared",
        grant_id: (g.relay_ref.kind == "grant").then(|| g.grant_id.clone()),
        share_id: (g.relay_ref.kind == "share").then(|| g.grant_id.clone()),
        scope: g.scope.clone(),
        thread_id: g.thread_id.clone(),
        workspace_id: g.workspace_id.clone(),
        workspace_scope: Some(g.workspace_scope.clone()),
        workspace_ids: g.workspace_ids.clone(),
        thread_access: g.thread_access.clone(),
        workspace_access: g.workspace_access.clone(),
        can_create_threads: g.can_create_threads,
    }
}
fn lineage(c: &Connection, target: Option<&str>) -> Result<Vec<String>> {
    let Some(target) = target else {
        return Ok(Vec::new());
    };
    Ok(c.prepare("WITH RECURSIVE ancestors(id,depth) AS (SELECT parent_thread_id,1 FROM threads WHERE id=?1 UNION ALL SELECT t.parent_thread_id,a.depth+1 FROM threads t JOIN ancestors a ON t.id=a.id WHERE a.depth<3) SELECT id FROM ancestors WHERE id IS NOT NULL ORDER BY depth")?
        .query_map([target],|r|r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub(crate) fn effective_access(
    state: &Supervisor,
    principal: &BrowserPrincipal,
    thread: Option<&str>,
    workspace: Option<&str>,
) -> Result<Option<EffectiveAccess>> {
    if principal.is_owner() {
        return Ok(Some(access::owner_access()));
    }
    state.db.with(|c| {
        let ancestors = lineage(c, thread)?;
        let rows: Vec<_> = live_rows(c, owner_trust::now())?
            .into_iter()
            .filter(|r| {
                Some(&r.grant.grantee.user_id) == principal.grantee_user_id.as_ref()
                    && r.grant.grantee.account_key_id == principal.account_key_id
            })
            .collect();
        let shares: Vec<_> = rows
            .iter()
            .filter(|r| r.grant.relay_ref.kind == "share")
            .map(|r| row_access(&r.grant))
            .collect();
        let grants: Vec<_> = rows
            .iter()
            .filter(|r| r.grant.relay_ref.kind == "grant")
            .map(|r| row_access(&r.grant))
            .collect();
        Ok(access::effective_access_from_rows(
            false, &shares, &grants, thread, workspace, &ancestors,
        ))
    })
}
pub(crate) fn owner_only(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    path.starts_with("/api/transport/trust")
        || path.starts_with("/api/transport/device-certificate")
        || path.starts_with("/api/transport/grants")
        || path.starts_with("/api/ports")
        || path.starts_with("/api/port-mappings")
        || path.starts_with("/api/config")
        || path.starts_with("/api/management")
        || path.starts_with("/api/settings")
        // Activity across every thread on the device, not one a grant names.
        || path.starts_with("/api/thread-activity")
}
pub(crate) fn authorize_path(
    state: &Supervisor,
    p: &BrowserPrincipal,
    method: &str,
    path: &str,
) -> Result<()> {
    if p.is_owner() {
        return Ok(());
    }
    if owner_only(path) {
        return owner_trust::reject(403, "owner_required");
    }
    let thread = access::resource_id_from_path(path, "threads");
    let workspace = access::resource_id_from_path(path, "workspaces");
    if effective_access(state, p, thread.as_deref(), workspace.as_deref())?
        .is_some_and(|a| access::access_allows(&a, method, path))
    {
        Ok(())
    } else {
        owner_trust::reject(403, "grant_denied")
    }
}
pub(crate) fn websocket_message_allowed(
    state: &Supervisor,
    p: &BrowserPrincipal,
    configured_thread: Option<&str>,
    message: &Value,
) -> bool {
    if p.is_owner() {
        return true;
    }
    let kind = message["type"].as_str().unwrap_or_default();
    let shell_thread = message["shellId"]
        .as_str()
        .and_then(|id| crate::shells::hub().get(id).ok())
        .and_then(|s| s["threadId"].as_str().map(str::to_owned));
    if kind.starts_with("shell.") && shell_thread.is_none() {
        return false;
    }
    let target = shell_thread
        .as_deref()
        .or(configured_thread)
        .or(message["threadId"].as_str());
    if configured_thread.zip(target).is_some_and(|(a, b)| a != b) {
        return false;
    }
    effective_access(state, p, target, None)
        .ok()
        .flatten()
        .is_some_and(|a| access::client_message_allowed(&a, kind))
}
pub(crate) fn websocket_event_allowed(
    state: &Supervisor,
    p: &BrowserPrincipal,
    configured_thread: Option<&str>,
    event: &Value,
) -> bool {
    if p.is_owner() {
        return true;
    }
    let shell = event["shellId"].as_str();
    let shell_thread = shell
        .and_then(|id| crate::shells::hub().get(id).ok())
        .and_then(|s| s["threadId"].as_str().map(str::to_owned));
    let thread = shell_thread
        .as_deref()
        .or(event["threadId"].as_str())
        .or(event["payload"]["threadId"].as_str());
    let mut scope_event = event.clone();
    if let Some(thread) = thread {
        scope_event["threadId"] = json!(thread);
    }
    if !access::outbound_event_in_scope(configured_thread, None, &scope_event) {
        return false;
    }
    if shell.is_some() && thread.is_none() {
        return false;
    }
    let control = matches!(
        event["type"].as_str(),
        Some("supervisor.connected" | "supervisor.pong")
    );
    if !control && thread.is_none() {
        return false;
    }
    effective_access(state, p, thread.or(configured_thread), None)
        .ok()
        .flatten()
        .is_some_and(|a| {
            if shell.is_some() {
                a.thread_access == "control"
            } else {
                control || thread.is_some()
            }
        })
}
pub(crate) async fn list_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<owner_trust::EncryptedOwnerRequest>,
    Extension(p): Extension<BrowserPrincipal>,
) -> Response {
    owner_trust::response(if p.is_owner() {
        state.db.with(|c| Ok(json!({"grants":rows(c)?})))
    } else {
        owner_trust::reject(403, "owner_required")
    })
}
pub(crate) async fn put_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<owner_trust::EncryptedOwnerRequest>,
    Extension(p): Extension<BrowserPrincipal>,
    Path(id): Path<String>,
    input: Result<Json<Grant>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(g)) = input else {
        return owner_trust::response(owner_trust::reject(400, "invalid_grant"));
    };
    let result = put(&state.db, &id, &g, &p, owner_trust::now());
    if result.is_ok() {
        crate::secure_transport::invalidate_unauthorized_channels(&state);
    }
    owner_trust::response(result)
}
pub(crate) async fn delete_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<owner_trust::EncryptedOwnerRequest>,
    Extension(p): Extension<BrowserPrincipal>,
    Path(id): Path<String>,
) -> Response {
    let result = if p.is_owner() {
        delete(&state.db, &id, owner_trust::now())
    } else {
        owner_trust::reject(403, "owner_required")
    };
    if result.is_ok() {
        crate::secure_transport::invalidate_unauthorized_channels(&state);
    }
    owner_trust::response(result)
}

#[cfg(test)]
#[path = "grants_tests.rs"]
pub(crate) mod tests;
