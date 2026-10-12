//! Account-key authority pinned locally; relay metadata grants no authority.
use anyhow::{ensure, Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hmac::{Hmac, Mac};
use p256::{
    ecdsa::{signature::Verifier, Signature, VerifyingKey},
    pkcs8::DecodePublicKey,
};
use pockymoe_runtime::{
    db::{validate_database_path, Database},
    Supervisor,
};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

const GRANTEE_REVOCATION_LIMIT: u64 = 1024;

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub(crate) fn hash(bytes: impl AsRef<[u8]>) -> String {
    B64.encode(Sha256::digest(bytes.as_ref()))
}
pub(crate) fn account_key_id(public_key: &str) -> Result<String> {
    let key = B64.decode(public_key)?;
    VerifyingKey::from_public_key_der(&key)?;
    ensure!(
        B64.encode(&key) == public_key,
        "Noncanonical account public key"
    );
    Ok(hash(key))
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinnedAccount {
    pub account_key_id: String,
    pub account_public_key: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserCertificate {
    pub version: u32,
    pub account_key_id: String,
    pub browser_id: String,
    pub public_key: String,
    pub created_at: i64,
    pub signature: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceCertificate {
    pub version: u32,
    pub account_key_id: String,
    pub device_id: String,
    pub identity_key: String,
    pub created_at: i64,
    pub signature: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Revocation {
    pub version: u32,
    pub account_key_id: String,
    pub kind: String,
    pub subject_id: String,
    pub revoked_at: i64,
    pub signature: String,
}
pub(crate) fn browser_signing_input(c: &BrowserCertificate) -> String {
    format!(
        "pockymoe-acct-browser-v1\n{}\n{}\n{}\n{}",
        c.account_key_id, c.browser_id, c.public_key, c.created_at
    )
}
pub(crate) fn device_signing_input(c: &DeviceCertificate) -> String {
    format!(
        "pockymoe-acct-device-v1\n{}\n{}\n{}\n{}",
        c.account_key_id, c.device_id, c.identity_key, c.created_at
    )
}
pub(crate) fn revocation_signing_input(r: &Revocation) -> String {
    format!(
        "pockymoe-acct-revoke-v1\n{}\n{}\n{}\n{}",
        r.account_key_id, r.kind, r.subject_id, r.revoked_at
    )
}
fn verify_signature(public: &str, input: &str, signature: &str) -> Result<()> {
    VerifyingKey::from_public_key_der(&B64.decode(public)?)?.verify(
        input.as_bytes(),
        &Signature::from_slice(&B64.decode(signature)?)?,
    )?;
    Ok(())
}
pub(crate) fn verify_browser_certificate(
    account: &PinnedAccount,
    c: &BrowserCertificate,
) -> Result<()> {
    ensure!(
        c.version == 1
            && c.account_key_id == account.account_key_id
            && account_key_id(&account.account_public_key)? == account.account_key_id
            && account_key_id(&c.public_key)? == c.browser_id,
        "Invalid browser certificate"
    );
    verify_signature(
        &account.account_public_key,
        &browser_signing_input(c),
        &c.signature,
    )
}
fn verify_device(
    account: &PinnedAccount,
    c: &DeviceCertificate,
    device: &str,
    identity: &str,
) -> Result<()> {
    ensure!(
        c.version == 1
            && c.account_key_id == account.account_key_id
            && account_key_id(&account.account_public_key)? == account.account_key_id
            && c.device_id == device
            && c.identity_key == identity,
        "Invalid device certificate"
    );
    VerifyingKey::from_sec1_bytes(&B64.decode(identity)?)?;
    verify_signature(
        &account.account_public_key,
        &device_signing_input(c),
        &c.signature,
    )
}
fn verify_revocation(account: &PinnedAccount, r: &Revocation) -> Result<()> {
    ensure!(
        r.version == 1
            && r.account_key_id == account.account_key_id
            && matches!(r.kind.as_str(), "browser" | "device")
            && !r.subject_id.is_empty()
            && r.subject_id.len() <= 256
            && !r.subject_id.contains(['\n', '\r']),
        "Invalid revocation"
    );
    verify_signature(
        &account.account_public_key,
        &revocation_signing_input(r),
        &r.signature,
    )
}
fn pinned_in(c: &Connection) -> Result<Option<PinnedAccount>> {
    Ok(c.query_row("SELECT account_key_id,account_public_key FROM device_owner_trust WHERE singleton=1 AND account_key_id IS NOT NULL",[],|r|Ok(PinnedAccount{account_key_id:r.get(0)?,account_public_key:r.get(1)?})).optional()?)
}
pub(crate) fn pinned_account(state: &Supervisor) -> Result<Option<PinnedAccount>> {
    state.db.with(pinned_in)
}
pub(crate) fn bound(state: &Supervisor) -> bool {
    pinned_account(state).map(|p| p.is_some()).unwrap_or(true)
}
pub(crate) fn strict(state: &Supervisor) -> bool {
    state
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT strict FROM device_owner_trust WHERE singleton=1",
                [],
                |r| r.get::<_, bool>(0),
            )?)
        })
        .unwrap_or(true)
}
pub(crate) fn hint(state: &Supervisor) -> Result<Value> {
    state.db.with(|c| { let pin=pinned_in(c)?; Ok(json!({"bound":pin.is_some(),"strict":c.query_row("SELECT strict FROM device_owner_trust WHERE singleton=1",[],|r|r.get::<_,bool>(0))?,"accountKeyId":pin.map(|p|p.account_key_id),"capabilities":["remote-bind"]})) })
}
pub(crate) fn device_id(c: &Connection) -> Result<String> {
    c.query_row(
        "SELECT relay_device_id FROM device_owner_trust WHERE singleton=1",
        [],
        |r| r.get::<_, Option<String>>(0),
    )?
    .filter(|id| !id.is_empty())
    .context("Device has not connected to the relay yet")
}
#[derive(Debug, thiserror::Error)]
#[error("Bound relay device id changed")]
pub(crate) struct DeviceIdMismatch;
pub(crate) fn relay_status(state: &Supervisor) -> Value {
    state
        .db
        .get_kv("relayTrustStatus")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or(Value::Null)
}
pub(crate) fn mismatch_retry(state: &Supervisor, delay: std::time::Duration) -> Result<()> {
    state.db.set_kv(
        "relayTrustStatus",
        &json!({"code":"device_id_mismatch","retryAt":now()+delay.as_millis() as i64}).to_string(),
    )
}
pub(crate) fn remember_device(state: &Supervisor, id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 256 && !id.contains(['\n', '\r']),
        "Invalid relay device id"
    );
    state.db.with(|c| {
        let previous: Option<String> = c.query_row(
            "SELECT relay_device_id FROM device_owner_trust WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if pinned_in(c)?.is_some() && previous.as_deref().is_some_and(|old|old != id) {
            c.execute("INSERT INTO kv(key,value) VALUES('relayTrustStatus',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [json!({"code":"device_id_mismatch","retryAt":null}).to_string()])?;
            return Err(DeviceIdMismatch.into());
        }
        c.execute("DELETE FROM kv WHERE key='relayTrustStatus'", [])?;
        c.execute(
            "UPDATE device_owner_trust SET relay_device_id=?1 WHERE singleton=1",
            [id],
        )?;
        Ok(())
    })
}
pub(crate) fn device_certificate(state: &Supervisor) -> Result<Option<DeviceCertificate>> {
    state.db.with(|c| {
        let saved: Option<String> = c
            .query_row(
                "SELECT certificate_json FROM device_certificates WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        saved.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    })
}
fn revoked(c: &Connection, account: &str, kind: &str, id: &str) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM account_revocations WHERE account_key_id=?1 AND kind=?2 AND subject_id=?3)",params![account,kind,id],|r|r.get(0))?)
}
pub(crate) fn device_revoked(state: &Supervisor, id: &str) -> Result<bool> {
    state.db.with(|c| match pinned_in(c)? {
        Some(p) => revoked(c, &p.account_key_id, "device", id),
        None => Ok(false),
    })
}
pub(crate) fn verify_device_certificate(
    state: &Supervisor,
    c: &DeviceCertificate,
    device: &str,
    identity: &str,
) -> Result<()> {
    let account = pinned_account(state)?.context("Device is not bound")?;
    verify_device(&account, c, device, identity)?;
    ensure!(!device_revoked(state, device)?, "Device is revoked");
    Ok(())
}
/// Constructed only after a valid signature over the outer HPKE ciphertext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrowserPrincipal {
    pub browser_id: String,
    pub account_key_id: String,
    pub grantee_user_id: Option<String>,
}
impl BrowserPrincipal {
    pub fn actor(&self) -> String {
        match &self.grantee_user_id {
            Some(user) => format!("grantee:{user}"),
            None => format!("owner:{}", self.browser_id),
        }
    }
    pub fn is_owner(&self) -> bool {
        self.grantee_user_id.is_none()
    }
    pub fn valid(&self, state: &Supervisor) -> bool {
        state
            .db
            .with(|c| {
                Ok(browser(c, &self.browser_id)?.is_some_and(|(_, p)| {
                    p.account_key_id == self.account_key_id
                        && p.grantee_user_id == self.grantee_user_id
                }))
            })
            .unwrap_or(false)
    }
}
#[derive(Clone)]
pub(crate) struct EncryptedOwnerRequest;
#[derive(Debug, thiserror::Error)]
#[error("{code}")]
struct Rejection {
    status: u16,
    code: &'static str,
}
pub(crate) fn reject<T>(status: u16, code: &'static str) -> Result<T> {
    Err(Rejection { status, code }.into())
}
pub(crate) fn rejection_code(error: &anyhow::Error) -> (u16, &'static str) {
    error
        .downcast_ref::<Rejection>()
        .map(|r| (r.status, r.code))
        .unwrap_or((400, "invalid_certificate"))
}
pub(crate) fn error(status: u16, code: &str) -> Value {
    json!({"statusCode":status,"headers":{"content-type":"application/json","cache-control":"no-store"},"body":json!({"code":code}).to_string()})
}
pub(crate) fn response(result: Result<Value>) -> Response {
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            let (status, code) = rejection_code(&e);
            (
                StatusCode::from_u16(status).unwrap(),
                Json(json!({"code":code})),
            )
                .into_response()
        }
    }
}
pub(crate) fn verify_request(
    public_key: &str,
    aad: &str,
    sealed: &[u8],
    signature: &str,
) -> Result<()> {
    verify_signature(
        public_key,
        &format!("pockymoe-request-v1\n{aad}\n{}", hash(sealed)),
        signature,
    )
}
fn browser(c: &Connection, id: &str) -> Result<Option<(String, BrowserPrincipal)>> {
    let row=c.query_row("SELECT public_key,account_key_id,user_id,role FROM browser_registrations WHERE browser_id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?))).optional()?;
    let Some((key, account, user, role)) = row else {
        return Ok(None);
    };
    if revoked(c, &account, "browser", id)? {
        return Ok(None);
    }
    let valid = if role == "owner" {
        pinned_in(c)?.is_some_and(|p| p.account_key_id == account) && user.is_none()
    } else {
        crate::grants::live_account(c, &account, now())?
            .is_some_and(|(_, live_user)| Some(live_user) == user)
    };
    Ok(valid.then(|| {
        (
            key,
            BrowserPrincipal {
                browser_id: id.into(),
                account_key_id: account,
                grantee_user_id: user,
            },
        )
    }))
}
pub(crate) fn authorize(
    state: &Supervisor,
    payload: &Value,
    aad: &str,
    sealed: &[u8],
) -> Result<BrowserPrincipal> {
    let Some(id) = payload["headers"]["x-rcd-session"].as_str() else {
        return reject(401, "browser_registration_required");
    };
    state.db.with(|c| {
        let registered: Option<String> = c
            .query_row(
                "SELECT account_key_id FROM browser_registrations WHERE browser_id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let revoked_browser = if let Some(account) = registered {
            revoked(c, &account, "browser", id)?
        } else {
            match pinned_in(c)? {
                Some(owner) => revoked(c, &owner.account_key_id, "browser", id)?,
                None => false,
            }
        };
        if revoked_browser {
            return reject(401, "browser_revoked");
        }
        let Some((key, principal)) = browser(c, id)? else {
            return reject(401, "browser_registration_required");
        };
        let signature = payload["headers"]["x-rcd-signature"].as_str().unwrap_or("");
        if verify_request(&key, aad, sealed, signature).is_err() {
            return reject(401, "browser_registration_required");
        }
        c.execute(
            "UPDATE browser_registrations SET last_seen_at=?1 WHERE browser_id=?2",
            params![now(), id],
        )?;
        Ok(principal)
    })
}
fn register(db: &Database, certificate: &BrowserCertificate) -> Result<Value> {
    db.with(|c| {
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let owner=pinned_in(&tx)?.context("Device is not bound")?;
        let (account,user)=if owner.account_key_id==certificate.account_key_id {(owner,None)} else {
            let Some((account,user))=crate::grants::live_account(&tx,&certificate.account_key_id,now())? else{return reject(403,"account_mismatch")};
            (account,Some(user))
        };
        if revoked(&tx,&account.account_key_id,"browser",&certificate.browser_id)? {return reject(401,"browser_revoked")}
        verify_browser_certificate(&account,certificate)?;
        let role=if user.is_none(){"owner"}else{"grantee"};
        if let Some((existing_account,existing_user,existing_role))=tx.query_row("SELECT account_key_id,user_id,role FROM browser_registrations WHERE browser_id=?1",[&certificate.browser_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?))).optional()? {
            if existing_account!=account.account_key_id || existing_user!=user {
                if role=="owner" && existing_role=="grantee" {
                    // A recipient can certify a known public key under its own
                    // account, but cannot reserve that id against its owner.
                    tx.execute("DELETE FROM browser_registrations WHERE browser_id=?1",[&certificate.browser_id])?;
                } else {return reject(403,"account_mismatch")}
            }
        }
        if user.is_some() && !tx.query_row("SELECT EXISTS(SELECT 1 FROM browser_registrations WHERE browser_id=?1)", [&certificate.browser_id], |r| r.get::<_,bool>(0))? {
            let count: u64 = tx.query_row("SELECT count(*) FROM browser_registrations WHERE account_key_id=?1 AND role='grantee'", [&account.account_key_id], |r| r.get(0))?;
            if count >= 32 { return reject(429,"browser_limit") }
        }
        tx.execute("INSERT INTO browser_registrations(browser_id,public_key,role,account_key_id,user_id,registered_at,last_seen_at) VALUES(?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(browser_id) DO UPDATE SET last_seen_at=excluded.last_seen_at",params![certificate.browser_id,certificate.public_key,role,account.account_key_id,user,now()])?;
        tx.commit()?; Ok(json!({"browserId":certificate.browser_id,"role":role}))
    })
}
pub(crate) fn enrollment_mac(
    secret: &[u8],
    device: &str,
    identity: &str,
    account: &str,
) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)?;
    mac.update(format!("pockymoe-acct-enroll-v1\n{device}\n{identity}\n{account}").as_bytes());
    Ok(B64.encode(mac.finalize().into_bytes()))
}
fn certificate_matches(c: &Connection, account: &PinnedAccount, identity: &str) -> Result<bool> {
    let saved: Option<String> = c
        .query_row(
            "SELECT certificate_json FROM device_certificates WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let device: Option<String> = c.query_row(
        "SELECT relay_device_id FROM device_owner_trust WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let (Some(saved), Some(device)) = (saved, device) else {
        return Ok(false);
    };
    let Ok(cert) = serde_json::from_str::<DeviceCertificate>(&saved) else {
        return Ok(false);
    };
    Ok(verify_device(account, &cert, &device, identity).is_ok()
        && !revoked(c, &account.account_key_id, "device", &device)?)
}
fn enrollment(state: &Supervisor) -> Result<Value> {
    let identity = crate::secure_transport::transport(state)?.identity_key();
    state.db.with(|c| {
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let pending:Option<Vec<u8>>=tx.query_row("SELECT enroll_secret FROM device_owner_trust WHERE singleton=1",[],|r|r.get(0))?;
        let Some(secret)=pending else{return reject(404,"enrollment_missing")};
        let account=pinned_in(&tx)?.context("Device is not bound")?;
        if certificate_matches(&tx,&account,&identity)? {
            tx.execute("UPDATE device_owner_trust SET enroll_secret=NULL WHERE singleton=1", [])?;
            tx.commit()?;
            return reject(404,"enrollment_missing");
        }
        let device=device_id(&tx)?;
        let result=json!({"deviceId":device,"identityKey":identity,"accountKeyId":account.account_key_id,"mac":enrollment_mac(&secret,&device,&identity,&account.account_key_id)?});
        tx.commit()?; Ok(result)
    })
}
fn install_certificate(state: &Supervisor, cert: &DeviceCertificate) -> Result<Value> {
    let identity = crate::secure_transport::transport(state)?.identity_key();
    state.db.with(|c| {
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let pin=pinned_in(&tx)?.context("Device is not bound")?;
        verify_device(&pin,cert,&device_id(&tx)?,&identity)?;
        ensure!(!revoked(&tx,&pin.account_key_id,"device",&cert.device_id)?,"Device is revoked");
        tx.execute("INSERT INTO device_certificates(singleton,certificate_json) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET certificate_json=excluded.certificate_json",[serde_json::to_string(cert)?])?;
        tx.execute("UPDATE device_owner_trust SET enroll_secret=NULL WHERE singleton=1",[])?;
        tx.commit()?; Ok(json!({"certificate":cert}))
    })
}
#[derive(Deserialize)]
pub(crate) struct Revocations {
    pub revocations: Vec<Value>,
}
#[cfg(test)]
pub(crate) fn apply_revocations(state: &Supervisor, entries: &[Revocation]) -> Result<Value> {
    let values = entries
        .iter()
        .map(serde_json::to_value)
        .collect::<serde_json::Result<Vec<_>>>()?;
    apply_revocation_values(state, &values)
}
pub(crate) fn apply_revocation_values(state: &Supervisor, entries: &[Value]) -> Result<Value> {
    ensure!(entries.len() <= 4096, "Too many revocations");
    state.db.with(|c| {
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let pin=pinned_in(&tx)?.context("Device is not bound")?;
        for value in entries {
            let Ok(r) = serde_json::from_value::<Revocation>(value.clone()) else { continue };
            let account = if r.account_key_id == pin.account_key_id {
                pin.clone()
            } else if r.kind == "browser" {
                let Some((account, _)) = crate::grants::live_account(&tx, &r.account_key_id, now())? else { continue };
                account
            } else { continue };
            if verify_revocation(&account, &r).is_err() { continue }

            if r.account_key_id != pin.account_key_id && !revoked(&tx,&r.account_key_id,&r.kind,&r.subject_id)? {
                let count: u64 = tx.query_row("SELECT count(*) FROM account_revocations WHERE account_key_id=?1", [&r.account_key_id], |row| row.get(0))?;
                if count >= GRANTEE_REVOCATION_LIMIT { return reject(429,"revocation_limit") }
            }

            tx.execute("INSERT INTO account_revocations(account_key_id,kind,subject_id,revocation_json) VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",params![r.account_key_id,r.kind,r.subject_id,serde_json::to_string(&r)?])?;
            if r.kind=="browser" {
                tx.execute("DELETE FROM browser_registrations WHERE browser_id=?1 AND account_key_id=?2",params![r.subject_id,r.account_key_id])?;
                tx.execute("DELETE FROM preview_leases WHERE browser_id=?1 AND account_key_id=?2",params![r.subject_id,r.account_key_id])?;
                // Port approvals belong to owners; a grantee using the same browser
                // public key cannot revoke an owner's approvals.
                if r.account_key_id == pin.account_key_id {
                    crate::ports::remove_browser_mappings(&tx,&r.subject_id)?;
                }
            } else {
                tx.execute("DELETE FROM peer_certificates WHERE device_id=?1",[&r.subject_id])?;
            }
        }
        tx.commit()?; Ok(())
    })?;
    crate::ports::refresh_owner_permissions(state);
    crate::secure_transport::invalidate_unauthorized_channels(state);
    Ok(json!({}))
}
fn trust(state: &Supervisor, current: &str) -> Result<Value> {
    state.db.with(|c| {
        let pin=pinned_in(c)?;
        let browsers=c.prepare("SELECT browser_id,role,user_id,registered_at,last_seen_at FROM browser_registrations ORDER BY registered_at,browser_id")?.query_map([],|r|{let id:String=r.get(0)?;Ok(json!({"browserId":id,"role":r.get::<_,String>(1)?,"userId":r.get::<_,Option<String>>(2)?,"registeredAt":r.get::<_,i64>(3)?,"lastSeenAt":r.get::<_,i64>(4)?,"current":id==current}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
        let cert:Option<String>=c.query_row("SELECT certificate_json FROM device_certificates WHERE singleton=1",[],|r|r.get(0)).optional()?;
        Ok(json!({"deviceId":device_id(c)?,"bound":pin.is_some(),"strict":c.query_row("SELECT strict FROM device_owner_trust WHERE singleton=1",[],|r|r.get::<_,bool>(0))?,"accountKeyId":pin.map(|p|p.account_key_id),"certificate":cert.map(|s|serde_json::from_str::<Value>(&s)).transpose()?,"browsers":browsers,"revocations":c.query_row("SELECT count(*) FROM account_revocations",[],|r|r.get::<_,u64>(0))?}))
    })
}
pub(crate) fn bootstrap(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("POST", "/api/transport/bind")
            | ("POST", "/api/transport/browser")
            | ("GET", "/api/transport/enrollment")
            | ("PUT", "/api/transport/revocations")
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BindInput {
    account_public_key: String,
    enroll_secret: String,
    web_origin: Option<String>,
}
pub(crate) async fn bind_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    relay: Option<Extension<crate::auth::TrustedRelayForward>>,
    peer: Option<Extension<crate::auth::PeerCaller>>,
    principal: Option<Extension<BrowserPrincipal>>,
    input: Result<Json<BindInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if relay.is_none() || peer.is_some() || principal.is_some_and(|Extension(p)| !p.is_owner()) {
        return response(reject(403, "owner_required"));
    }
    let result = input
        .map_err(|_| anyhow::anyhow!("Invalid binding request"))
        .and_then(|Json(input)| {
            state.db.with(|c| {
                bind_on_connection(
                    c,
                    &state.config.database_url,
                    &input.account_public_key,
                    &input.enroll_secret,
                    input.web_origin.as_deref(),
                    true,
                )
            })
        })
        .map_err(|error| {
            if error.is::<Rejection>() {
                error
            } else {
                Rejection {
                    status: 400,
                    code: "invalid_bind",
                }
                .into()
            }
        });
    response(result)
}
pub(crate) async fn browser_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    input: Result<Json<BrowserCertificate>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let result = input
        .map_err(|_| anyhow::anyhow!("Invalid certificate"))
        .and_then(|Json(c)| register(&state.db, &c));
    if result.is_ok() {
        crate::secure_transport::invalidate_unauthorized_channels(&state);
    }
    response(result)
}
pub(crate) async fn enrollment_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
) -> Response {
    response(enrollment(&state))
}
pub(crate) async fn certificate_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(p): Extension<BrowserPrincipal>,
    input: Result<Json<DeviceCertificate>, axum::extract::rejection::JsonRejection>,
) -> Response {
    response(if !p.is_owner() {
        reject(403, "owner_required")
    } else {
        input
            .map_err(|_| anyhow::anyhow!("Invalid certificate"))
            .and_then(|Json(c)| install_certificate(&state, &c))
    })
}
pub(crate) async fn revocations_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    input: Result<Json<Revocations>, axum::extract::rejection::JsonRejection>,
) -> Response {
    response(
        input
            .map_err(|_| anyhow::anyhow!("Invalid revocations"))
            .and_then(|Json(r)| apply_revocation_values(&state, &r.revocations)),
    )
}
pub(crate) async fn trust_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(p): Extension<BrowserPrincipal>,
) -> Response {
    response(if p.is_owner() {
        trust(&state, &p.browser_id)
    } else {
        reject(403, "owner_required")
    })
}
fn local_connection(path: &Path) -> Result<Connection> {
    validate_database_path(path)?;
    if !path.exists() {
        drop(Database::open(path)?);
    }
    pockymoe_runtime::db::secure_database_file(path)?;
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    pockymoe_runtime::db::ensure_device_owner_trust(&c)?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    Ok(c)
}
pub fn validate_web_origin(origin: &str) -> Result<String> {
    let u = url::Url::parse(origin).map_err(|_| anyhow::anyhow!("Invalid Web origin"))?;
    ensure!(
        (u.scheme() == "https" || (u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none(),
        "Web origin must use HTTPS (or HTTP on loopback), without credentials, path, query or fragment"
    );
    Ok(u.origin().ascii_serialization())
}
pub fn validate_binding(account: &str, enroll: &str) -> Result<()> {
    account_key_id(account).map_err(|_| anyhow::anyhow!("Invalid account public key"))?;
    ensure!(
        B64.decode(enroll)
            .is_ok_and(|b| b.len() == 32 && B64.encode(b) == enroll),
        "Enrollment secret must be 32 bytes encoded as base64url"
    );
    Ok(())
}
pub fn bind_account(
    database: &Path,
    account: &str,
    enroll: &str,
    web_origin: Option<&str>,
) -> Result<Value> {
    validate_binding(account, enroll)?;
    let origin = web_origin.map(validate_web_origin).transpose()?;
    let c = local_connection(database)?;
    bind_on_connection(&c, database, account, enroll, origin.as_deref(), false)
}
fn bind_on_connection(
    c: &Connection,
    database: &Path,
    account: &str,
    enroll: &str,
    web_origin: Option<&str>,
    only_unbound: bool,
) -> Result<Value> {
    let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
    let pin = pinned_in(&tx)?;
    if only_unbound && pin.is_some() {
        return reject(409, "already_bound");
    }
    validate_binding(account, enroll)?;
    let origin = web_origin.map(validate_web_origin).transpose()?;
    let id = account_key_id(account)?;
    if let Some(pin) = pin {
        ensure!(
            pin.account_key_id == id && pin.account_public_key == account,
            "Device is bound to another account key; use pockymoe bind --reset to replace it"
        );
    }
    let device = if only_unbound {
        Some(device_id(&tx)?)
    } else {
        None
    };
    let identity = crate::secure_transport::identity_key_for_database(database)?;
    let pin = PinnedAccount {
        account_key_id: id.clone(),
        account_public_key: account.into(),
    };
    let pending = if certificate_matches(&tx, &pin, &identity)? {
        None
    } else {
        // A certificate for an identity that was lost/replaced must not be
        // advertised while its replacement is awaiting owner enrollment.
        tx.execute("DELETE FROM device_certificates", [])?;
        Some(B64.decode(enroll)?)
    };
    tx.execute("UPDATE device_owner_trust SET account_key_id=?1,account_public_key=?2,enroll_secret=?3,bound_at=COALESCE(bound_at,?4),web_origin=COALESCE(?5,web_origin),strict=1 WHERE singleton=1",params![id,account,pending,now(),origin])?;
    pockymoe_runtime::publications::delete_keyless_publications(&tx)?;
    tx.commit()?;
    if only_unbound {
        Ok(json!({"bound":true,"accountKeyId":id,"deviceId":device}))
    } else {
        Ok(json!({"bound":true,"strict":true,"accountKeyId":id}))
    }
}
pub fn binding_status(database: &Path) -> Result<Value> {
    validate_database_path(database)?;
    let unbound = json!({"bound":false,"strict":false,"accountKeyId":null});
    if !database.exists() {
        return Ok(unbound);
    }
    let c = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has_trust: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='device_owner_trust')", [], |r|r.get(0))?;
    if !has_trust {
        return Ok(unbound);
    }
    let pin = pinned_in(&c)?;
    let strict = c.query_row(
        "SELECT strict FROM device_owner_trust WHERE singleton=1",
        [],
        |r| r.get::<_, bool>(0),
    )?;
    let mut status =
        json!({"bound":pin.is_some(),"strict":strict,"accountKeyId":pin.map(|p|p.account_key_id)});
    let origin: Option<String> = c.query_row(
        "SELECT web_origin FROM device_owner_trust WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if let Some(origin) = origin {
        status["webOrigin"] = json!(origin);
    }
    let relay: Option<String> = c
        .query_row(
            "SELECT value FROM kv WHERE key='relayTrustStatus'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(relay) = relay {
        status["relayStatus"] = serde_json::from_str(&relay)?;
    }
    Ok(status)
}
pub fn reset_account_trust(database: &Path) -> Result<()> {
    let c = local_connection(database)?;
    let tx = Transaction::new_unchecked(&c, TransactionBehavior::Immediate)?;
    let saved: Option<String> = tx
        .query_row(
            "SELECT value FROM kv WHERE key='devicePortMappings'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(saved) = saved {
        let mut mappings: Vec<Value> = serde_json::from_str(&saved)?;
        mappings.retain(|m| {
            !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM owner_port_mappings WHERE mapping_id=?1)",
                [m["id"].as_str().unwrap_or("")],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(true)
        });
        tx.execute(
            "UPDATE kv SET value=?1 WHERE key='devicePortMappings'",
            [serde_json::to_string(&mappings)?],
        )?;
    }
    tx.execute_batch("DELETE FROM preview_leases WHERE browser_id IS NOT NULL; DELETE FROM browser_registrations; DELETE FROM device_certificates; DELETE FROM device_grants; DELETE FROM peer_certificates; DELETE FROM owner_port_mappings; DELETE FROM kv WHERE key='relayTrustStatus'; UPDATE device_owner_trust SET account_key_id=NULL,account_public_key=NULL,enroll_secret=NULL,bound_at=NULL,web_origin=NULL,strict=0 WHERE singleton=1;")?;
    tx.commit()?;
    Ok(())
}

// Peer authority is scoped to the locally pinned account and permanent revocations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerPrincipal {
    device_id: String,
    identity_key: String,
    account_key_id: String,
}
impl PeerPrincipal {
    pub(crate) fn valid(&self, state: &Supervisor, device: &str) -> bool {
        device == self.device_id && state.db.with(|c| {
            Ok(pinned_in(c)?.is_some_and(|p| p.account_key_id == self.account_key_id)
                && !revoked(c, &self.account_key_id, "device", device)?
                && c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM peer_certificates WHERE device_id=?1 AND identity_key=?2 AND account_key_id=?3)",
                    params![device, self.identity_key, self.account_key_id],
                    |r| r.get::<_, bool>(0),
                )?)
        }).unwrap_or(false)
    }
}
fn verify_peer_certificate_in(
    c: &Connection,
    value: Value,
    device: &str,
    identity: Option<&str>,
) -> Result<PeerPrincipal> {
    let cert: DeviceCertificate = serde_json::from_value(value)?;
    let pin = pinned_in(c)?.context("Device is not bound")?;
    verify_device(&pin, &cert, device, identity.unwrap_or(&cert.identity_key))?;
    ensure!(
        !revoked(c, &pin.account_key_id, "device", device)?,
        "Device is revoked"
    );
    Ok(PeerPrincipal {
        device_id: device.into(),
        identity_key: cert.identity_key,
        account_key_id: cert.account_key_id,
    })
}
pub(crate) fn verify_peer_certificate(
    state: &Supervisor,
    value: Value,
    device: &str,
    identity: Option<&str>,
) -> Result<PeerPrincipal> {
    let cert: DeviceCertificate = serde_json::from_value(value)?;
    verify_device_certificate(state, &cert, device, identity.unwrap_or(&cert.identity_key))?;
    Ok(PeerPrincipal {
        device_id: device.into(),
        identity_key: cert.identity_key,
        account_key_id: cert.account_key_id,
    })
}
pub(crate) fn peer_signing_input(device: &str, aad: &str, sealed: &[u8]) -> String {
    format!(
        "pockymoe-peer-request-v1\n{device}\n{aad}\n{}",
        hash(sealed)
    )
}
pub(crate) fn authorize_peer(
    state: &Supervisor,
    caller: &crate::auth::PeerCaller,
    payload: &Value,
    aad: &str,
    sealed: &[u8],
) -> Result<PeerPrincipal> {
    ensure!(
        payload["headers"]["x-rcd-peer-device"].as_str() == Some(&caller.device_id),
        "Peer caller mismatch"
    );
    let principal = state.db.with(|c| {
        Ok(c.query_row(
            "SELECT identity_key,account_key_id FROM peer_certificates WHERE device_id=?1",
            [&caller.device_id],
            |r| {
                Ok(PeerPrincipal {
                    device_id: caller.device_id.clone(),
                    identity_key: r.get(0)?,
                    account_key_id: r.get(1)?,
                })
            },
        )?)
    })?;
    ensure!(
        principal.valid(state, &caller.device_id),
        "Peer is revoked or belongs to another account"
    );
    VerifyingKey::from_sec1_bytes(&B64.decode(&principal.identity_key)?)?.verify(
        peer_signing_input(&caller.device_id, aad, sealed).as_bytes(),
        &Signature::from_slice(
            &B64.decode(
                payload["headers"]["x-rcd-peer-signature"]
                    .as_str()
                    .unwrap_or(""),
            )?,
        )?,
    )?;
    Ok(principal)
}
pub(crate) async fn peer_certificate_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(caller): Extension<crate::auth::PeerCaller>,
    Json(value): Json<Value>,
) -> Response {
    response(state.db.with(|c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        if pinned_in(&tx)?.is_none() { return Ok(json!({})); }
        // Verify and store in one transaction so a concurrent revocation or
        // account reset cannot leave behind a new, already revoked registration.
        let proof = verify_peer_certificate_in(&tx, value, &caller.device_id, None)
            .map_err(|_| Rejection { status: 401, code: "peer_certificate_required" })?;
        tx.execute(
            "INSERT INTO peer_certificates(device_id,identity_key,account_key_id) VALUES(?1,?2,?3) ON CONFLICT(device_id) DO UPDATE SET identity_key=excluded.identity_key,account_key_id=excluded.account_key_id",
            params![proof.device_id, proof.identity_key, proof.account_key_id],
        )?;
        tx.commit()?;
        Ok(json!({}))
    }))
}
#[cfg(test)]
#[path = "peer_certificate_tests.rs"]
mod peer_tests;

#[cfg(test)]
#[path = "owner_trust_tests.rs"]
pub(crate) mod tests;
