//! Device-local owner authority. Relay metadata is never an owner credential.
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, State},
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
use rand::{rngs::OsRng, RngCore};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path as FsPath, sync::Arc};

const CLOCK_WINDOW: u64 = 5 * 60 * 1000;
const MAX_LIFETIME: i64 = 7 * 24 * 60 * 60 * 1000;
// Public session IDs and expiry times only; retained independently of credentials.
const REVOKED_SESSION_PREFIX: &str = "ownerRevokedSession:";
pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    B64.encode(Sha256::digest(bytes.as_ref()))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Credential {
    pub credential_id: String,
    pub public_key: String,
    pub algorithm: i32,
    pub rp_id: String,
    pub origin: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Assertion {
    pub credential_id: String,
    pub authenticator_data: String,
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    pub signature: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Registration {
    pub version: u32,
    pub session_public_key: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub nonce: String,
    pub assertion: Assertion,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pairing {
    pub version: u32,
    pub pairing_id: String,
    pub credential: Credential,
    pub assertion: Assertion,
    pub mac: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceCertificate {
    pub version: u32,
    pub device_id: String,
    pub identity_key: String,
    pub pairing_id: String,
    pub assertion: Assertion,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CertificateBundle {
    pub version: u32,
    pub device_id: String,
    pub certificates: Vec<DeviceCertificate>,
}
impl CertificateBundle {
    pub(crate) fn parse(value: Value) -> Result<Self> {
        if value.get("certificates").is_some() {
            Ok(serde_json::from_value(value)?)
        } else {
            let certificate: DeviceCertificate = serde_json::from_value(value)?;
            Ok(Self {
                version: certificate.version,
                device_id: certificate.device_id.clone(),
                certificates: vec![certificate],
            })
        }
    }
}
fn local_bundle(c: &Connection) -> Result<Option<CertificateBundle>> {
    let raw: Option<String> = c
        .query_row(
            "SELECT certificate_json FROM device_certificates WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| CertificateBundle::parse(serde_json::from_str(&s)?))
        .transpose()
}
pub(crate) fn certificate_bundle(state: &Supervisor) -> Result<Option<CertificateBundle>> {
    state.db.with(local_bundle)
}
fn store_bundle(c: &Connection, bundle: &CertificateBundle) -> Result<()> {
    c.execute("INSERT INTO device_certificates(singleton,certificate_json) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET certificate_json=excluded.certificate_json", [serde_json::to_string(bundle)?])?;
    Ok(())
}
fn append_certificate(c: &Connection, certificate: DeviceCertificate) -> Result<()> {
    let mut bundle = local_bundle(c)?.unwrap_or(CertificateBundle {
        version: 1,
        device_id: certificate.device_id.clone(),
        certificates: vec![],
    });
    ensure!(
        bundle.version == 1 && bundle.device_id == certificate.device_id,
        "invalid local certificate bundle"
    );
    bundle
        .certificates
        .retain(|entry| entry.assertion.credential_id != certificate.assertion.credential_id);
    bundle.certificates.push(certificate);
    store_bundle(c, &bundle)
}
/// Only local owner records confer trust; relay hints and grantee records never do.
pub(crate) fn verify_peer_bundle(
    state: &Supervisor,
    value: Value,
    device: &str,
    identity: Option<&str>,
) -> Result<PeerPrincipal> {
    let bundle = CertificateBundle::parse(value)?;
    ensure!(
        bundle.version == 1 && bundle.device_id == device,
        "peer certificate device mismatch"
    );
    state.db.with(|c| {
        for cert in bundle.certificates {
            if cert.version != 1
                || cert.device_id != device
                || identity.is_some_and(|key| key != cert.identity_key)
            {
                continue;
            }
            let Some(record) = credential(c, &cert.assertion.credential_id)? else {
                continue;
            };
            if verify(
                &record,
                &cert.assertion,
                &certificate_challenge(device, &cert.identity_key, &cert.pairing_id),
            )
            .is_err()
            {
                continue;
            }
            VerifyingKey::from_sec1_bytes(&B64.decode(&cert.identity_key)?)?;
            return Ok(PeerPrincipal {
                device_id: device.into(),
                identity_key: cert.identity_key,
                credential_id: record.credential_id,
            });
        }
        bail!("no certificate verifies against an owner credential")
    })
}
#[derive(Clone, Debug)]
pub(crate) struct PeerPrincipal {
    device_id: String,
    identity_key: String,
    credential_id: String,
}
impl PeerPrincipal {
    pub(crate) fn valid(&self, state: &Supervisor, device: &str) -> bool {
        device == self.device_id
            && state.db.with(|c| {
                Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM peer_certificates p JOIN owner_credentials c USING(credential_id) WHERE device_id=?1 AND identity_key=?2 AND credential_id=?3)",
                    params![self.device_id, self.identity_key, self.credential_id],
                    |r| r.get::<_, bool>(0),
                )?)
            }).unwrap_or(false)
    }
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
    let headers = &payload["headers"];
    ensure!(
        headers["x-rcd-peer-device"].as_str() == Some(caller.device_id.as_str()),
        "peer caller mismatch"
    );
    let principal = state.db.with(|c| {
        Ok(c.query_row("SELECT p.identity_key,p.credential_id FROM peer_certificates p JOIN owner_credentials c USING(credential_id) WHERE device_id=?1", [&caller.device_id], |r| Ok(PeerPrincipal { device_id: caller.device_id.clone(), identity_key: r.get(0)?, credential_id: r.get(1)? }))?)
    })?;
    let key = VerifyingKey::from_sec1_bytes(&B64.decode(&principal.identity_key)?)?;
    let sig = Signature::from_slice(
        &B64.decode(
            headers["x-rcd-peer-signature"]
                .as_str()
                .context("missing peer signature")?,
        )?,
    )?;
    key.verify(
        peer_signing_input(&caller.device_id, aad, sealed).as_bytes(),
        &sig,
    )?;
    Ok(principal)
}
pub(crate) async fn peer_certificate_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(caller): Extension<crate::auth::PeerCaller>,
    Json(value): Json<Value>,
) -> Response {
    response((|| {
        if !paired(&state) {
            return Ok(json!({}));
        }
        let proof =
            verify_peer_bundle(&state, value, &caller.device_id, None).map_err(|_| Rejection {
                status: 401,
                code: "peer_certificate_required",
            })?;
        state.db.with(|c| {
            c.execute("INSERT INTO peer_certificates(device_id,identity_key,credential_id) VALUES(?1,?2,?3) ON CONFLICT(device_id) DO UPDATE SET identity_key=excluded.identity_key,credential_id=excluded.credential_id", params![proof.device_id,proof.identity_key,proof.credential_id])?;
            Ok(json!({}))
        })
    })())
}

/// Constructed only after verifying the signature on the outer HPKE ciphertext.
#[derive(Clone, Debug)]
pub(crate) struct OwnerPrincipal {
    pub credential_id: String,
    pub session_id: String,
}
impl OwnerPrincipal {
    pub fn actor(&self) -> String {
        format!("owner:{}", self.credential_id)
    }
    pub fn valid(&self, state: &Supervisor) -> bool {
        session(&state.db, &self.session_id, now())
            .is_ok_and(|s| s.is_some_and(|s| s.credential_id == self.credential_id))
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
fn reject<T>(status: u16, code: &'static str) -> Result<T> {
    Err(Rejection { status, code }.into())
}
pub(crate) fn error(status: u16, code: &str) -> Value {
    json!({"statusCode":status,"headers":{"content-type":"application/json","cache-control":"no-store"},"body":json!({"code":code}).to_string()})
}
fn response(result: Result<Value>) -> Response {
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            let (status, code) = e
                .downcast_ref::<Rejection>()
                .map(|r| (r.status, r.code))
                .unwrap_or((400, "invalid_owner_proof"));
            (
                StatusCode::from_u16(status).unwrap(),
                Json(json!({"code":code})),
            )
                .into_response()
        }
    }
}

pub(crate) fn verify(record: &Credential, assertion: &Assertion, challenge: &[u8]) -> Result<()> {
    ensure!(
        assertion.credential_id == record.credential_id,
        "credential mismatch"
    );
    let client_bytes = B64.decode(&assertion.client_data_json)?;
    let client: Value = serde_json::from_slice(&client_bytes)?;
    ensure!(
        client["type"] == "webauthn.get"
            && client["challenge"] == B64.encode(challenge)
            && client["origin"] == record.origin,
        "client data mismatch"
    );
    ensure!(
        client.get("crossOrigin").is_none_or(|v| v == false),
        "cross-origin assertion"
    );
    let mut signed = B64.decode(&assertion.authenticator_data)?;
    ensure!(
        signed.len() >= 37
            && signed[..32] == Sha256::digest(record.rp_id.as_bytes())[..]
            && signed[32] & 0x05 == 0x05,
        "invalid authenticator data"
    );
    signed.extend_from_slice(&Sha256::digest(&client_bytes));
    let key = B64.decode(&record.public_key)?;
    let signature = B64.decode(&assertion.signature)?;
    match record.algorithm {
        -7 => VerifyingKey::from_public_key_der(&key)?
            .verify(&signed, &Signature::from_der(&signature)?)?,
        -257 => {
            let key = rsa::RsaPublicKey::from_public_key_der(&key)?;
            rsa::pkcs1v15::VerifyingKey::<Sha256>::new(key).verify(
                &signed,
                &rsa::pkcs1v15::Signature::try_from(signature.as_slice())?,
            )?;
        }
        _ => bail!("unsupported credential algorithm"),
    }
    Ok(())
}
pub(crate) fn session_challenge(r: &Registration) -> Vec<u8> {
    Sha256::digest(
        format!(
            "pockymoe-owner-session-v1\n{}\n{}\n{}\n{}",
            r.session_public_key, r.issued_at, r.expires_at, r.nonce
        )
        .as_bytes(),
    )
    .to_vec()
}
fn certificate_challenge(device: &str, identity: &str, pairing: &str) -> Vec<u8> {
    Sha256::digest(format!("pockymoe-device-cert-v1\n{device}\n{identity}\n{pairing}").as_bytes())
        .to_vec()
}
fn pairing_mac_input(device: &str, p: &Pairing) -> Result<String> {
    let c = &p.credential;
    Ok(format!(
        "pockymoe-pair-v1\n{device}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        p.pairing_id,
        c.credential_id,
        c.public_key,
        c.algorithm,
        c.rp_id,
        c.origin,
        hash(B64.decode(&p.assertion.client_data_json)?)
    ))
}
fn valid_mac(secret: &[u8], device: &str, p: &Pairing) -> bool {
    (|| -> Result<()> {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret)?;
        mac.update(pairing_mac_input(device, p)?.as_bytes());
        mac.verify_slice(&B64.decode(&p.mac)?)?;
        Ok(())
    })()
    .is_ok()
}
pub(crate) fn verify_request(
    public_key: &str,
    aad: &str,
    sealed: &[u8],
    signature: &str,
) -> Result<()> {
    let input = format!("pockymoe-request-v1\n{aad}\n{}", hash(sealed));
    VerifyingKey::from_public_key_der(&B64.decode(public_key)?)?.verify(
        input.as_bytes(),
        &Signature::from_slice(&B64.decode(signature)?)?,
    )?;
    Ok(())
}

pub(crate) fn strict(state: &Supervisor) -> bool {
    // Fail closed on a storage error; a failed DB read must not restore relay trust.
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
pub(crate) fn paired(state: &Supervisor) -> bool {
    state
        .db
        .with(|c| {
            Ok(
                c.query_row("SELECT EXISTS(SELECT 1 FROM owner_credentials)", [], |r| {
                    r.get::<_, bool>(0)
                })?,
            )
        })
        .unwrap_or(true)
}
pub(crate) fn hint(state: &Supervisor) -> Result<Value> {
    state.db.with(|c| Ok(json!({"paired":c.query_row("SELECT EXISTS(SELECT 1 FROM owner_credentials)",[],|r|r.get::<_,bool>(0))?,"strict":c.query_row("SELECT strict FROM device_owner_trust WHERE singleton=1",[],|r|r.get::<_,bool>(0))?})))
}
pub(crate) fn remember_device(state: &Supervisor, id: &str) -> Result<()> {
    ensure!(
        !id.is_empty() && id.len() <= 256 && !id.contains(['\n', '\r']),
        "invalid relay device id"
    );
    state.db.with(|c| {
        let previous: Option<String> = c.query_row(
            "SELECT relay_device_id FROM device_owner_trust WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        // A compromised greeting cannot replace an identity pinned by local pairing.
        ensure!(
            !strict_in(c)? || previous.as_deref() == Some(id),
            "paired relay device id changed"
        );
        c.execute(
            "UPDATE device_owner_trust SET relay_device_id=?1 WHERE singleton=1",
            [id],
        )?;
        Ok(())
    })
}
fn strict_in(c: &Connection) -> Result<bool> {
    Ok(c.query_row(
        "SELECT strict FROM device_owner_trust WHERE singleton=1",
        [],
        |r| r.get(0),
    )?)
}
fn device_id(c: &Connection) -> Result<String> {
    c.query_row("SELECT relay_device_id FROM device_owner_trust WHERE singleton=1",[],|r|r.get::<_,Option<String>>(0))?.filter(|id| !id.is_empty()).context("Device has not connected to the relay yet. Connect the Supervisor once before running pockymoe pair.")
}
fn credential(c: &Connection, id: &str) -> Result<Option<Credential>> {
    c.query_row(
        "SELECT record_json FROM owner_credentials WHERE credential_id=?1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .map(|s| Ok(serde_json::from_str(&s)?))
    .transpose()
}
struct OwnerSession {
    public_key: String,
    credential_id: String,
}
fn session(db: &Database, id: &str, time: i64) -> Result<Option<OwnerSession>> {
    db.with(|c| Ok(c.query_row("SELECT s.public_key,s.credential_id FROM owner_sessions s JOIN owner_credentials c USING(credential_id) WHERE session_id=?1 AND expires_at>?2",params![id,time],|r|Ok(OwnerSession {public_key:r.get(0)?,credential_id:r.get(1)?})).optional()?))
}
pub(crate) fn authorize(
    state: &Supervisor,
    payload: &Value,
    aad: &str,
    sealed: &[u8],
) -> Result<OwnerPrincipal> {
    let id = payload["headers"]["x-rcd-session"]
        .as_str()
        .context("missing owner session")?;
    let s = session(&state.db, id, now())?.context("unknown or expired owner session")?;
    verify_request(
        &s.public_key,
        aad,
        sealed,
        payload["headers"]["x-rcd-signature"]
            .as_str()
            .context("missing owner signature")?,
    )?;
    Ok(OwnerPrincipal {
        session_id: id.into(),
        credential_id: s.credential_id,
    })
}
fn check_owner_origin(c: &Connection, record: &Credential) -> Result<()> {
    // All credentials share the origin/RP established by the first pairing.
    // Checking every existing record also fails closed on previously mixed records.
    for existing in credentials(c)? {
        if existing["origin"] != record.origin || existing["rpId"] != record.rp_id {
            return reject(400, "owner_origin_mismatch");
        }
    }
    Ok(())
}
fn store_credential(c: &Connection, record: &Credential) -> Result<()> {
    if let Some(existing) = credential(c, &record.credential_id)? {
        ensure!(
            serde_json::to_value(existing)? == serde_json::to_value(record)?,
            "owner credential substitution"
        );
    } else {
        c.execute(
            "INSERT INTO owner_credentials(credential_id,record_json) VALUES(?1,?2)",
            params![record.credential_id, serde_json::to_string(record)?],
        )?;
    }
    Ok(())
}
fn pair(db: &Database, identity: &str, p: &Pairing, time: i64) -> Result<Value> {
    ensure!(
        p.version == 1 && B64.decode(&p.pairing_id)?.len() == 16,
        "invalid pairing version/id"
    );
    db.with(|c| {
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate)?;
        let device=device_id(&tx)?;
        let offer=tx.query_row("SELECT secret,web_origin,rp_id,expires_at,used_at,failures FROM pairing_offers WHERE pairing_id=?1",[&p.pairing_id],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,u32>(5)?))).optional()?;
        let Some((secret,origin,rp,expires,used,failures))=offer else { return reject(400,"invalid_pairing_offer"); };
        if used.is_some() || expires <= time || failures >= 5 { return reject(400,"invalid_pairing_offer"); }
        if !valid_mac(&secret,&device,p) {
            tx.execute("UPDATE pairing_offers SET failures=failures+1 WHERE pairing_id=?1",[&p.pairing_id])?;
            tx.commit()?;
            return reject(400,"invalid_pairing_mac");
        }
        ensure!(p.credential.origin == origin && p.credential.rp_id == rp,"pairing origin/rpId mismatch");
        check_owner_origin(&tx,&p.credential)?;
        verify(&p.credential,&p.assertion,&certificate_challenge(&device,identity,&p.pairing_id))?;
        let certificate=json!({"version":1,"deviceId":device,"identityKey":identity,"pairingId":p.pairing_id,"assertion":p.assertion});
        store_credential(&tx,&p.credential)?;
        append_certificate(&tx, serde_json::from_value(certificate.clone())?)?;
        tx.execute("UPDATE pairing_offers SET used_at=?1 WHERE pairing_id=?2",params![time,p.pairing_id])?;
        tx.execute("UPDATE device_owner_trust SET strict=1 WHERE singleton=1",[])?;
        tx.commit()?;
        Ok(json!({"paired":true,"certificate":certificate}))
    })
}
fn register(db: &Database, r: &Registration, time: i64) -> Result<Value> {
    // Persist GC even when the registration itself is rejected (e.g. expired).
    db.with(|c| collect_revoked_sessions(c, time))?;
    ensure!(
        r.version == 1
            && r.issued_at <= time.saturating_add(CLOCK_WINDOW as i64)
            && r.issued_at < r.expires_at
            && r.expires_at
                <= r.issued_at
                    .checked_add(MAX_LIFETIME)
                    .context("invalid session time")?
            && r.expires_at > time,
        "invalid session lifetime"
    );
    ensure!(B64.decode(&r.nonce)?.len() == 16, "invalid session nonce");
    let key = B64.decode(&r.session_public_key)?;
    VerifyingKey::from_public_key_der(&key)?;
    let id = hash(&key);
    db.with(|c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM kv WHERE key=?1 AND CAST(value AS INTEGER)>?2)",
            params![format!("{REVOKED_SESSION_PREFIX}{id}"), time],
            |r| r.get::<_, bool>(0),
        )? {
            return reject(401, "owner_session_revoked");
        }
        let record =
            credential(&tx, &r.assertion.credential_id)?.context("unknown owner credential")?;
        verify(&record, &r.assertion, &session_challenge(r))?;
        tx.execute(
            "INSERT INTO owner_sessions(session_id,public_key,credential_id,expires_at)
             VALUES(?1,?2,?3,?4) ON CONFLICT(session_id) DO UPDATE SET
             public_key=excluded.public_key,credential_id=excluded.credential_id,
             expires_at=excluded.expires_at",
            params![id, r.session_public_key, record.credential_id, r.expires_at],
        )?;
        tx.commit()?;
        Ok(json!({"sessionId":id,"expiresAt":r.expires_at}))
    })
}
fn credentials(c: &Connection) -> Result<Vec<Value>> {
    let rows = c
        .prepare("SELECT record_json FROM owner_credentials ORDER BY credential_id")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|s| Ok(serde_json::from_str(&s)?))
        .collect()
}
fn trust(db: &Database, principal: &OwnerPrincipal) -> Result<Value> {
    db.with(|c| {
        let sessions=c.prepare("SELECT session_id,credential_id,expires_at FROM owner_sessions WHERE expires_at>?1 ORDER BY session_id")?.query_map([now()],|r| {let id:String=r.get(0)?;Ok(json!({"sessionId":id,"credentialId":r.get::<_,String>(1)?,"expiresAt":r.get::<_,i64>(2)?,"current":id==principal.session_id}))})?.collect::<rusqlite::Result<Vec<_>>>()?;
        let certificate = local_bundle(c)?;
        let records=credentials(c)?;
        Ok(json!({"deviceId":device_id(c)?,"paired":!records.is_empty(),"strict":strict_in(c)?,"credentials":records,"sessions":sessions,"certificate":certificate}))
    })
}
#[derive(Deserialize)]
pub(crate) struct AddCredential {
    credential: Credential,
    assertion: Assertion,
    certificate: Option<Assertion>,
}
fn add(db: &Database, input: &AddCredential) -> Result<Value> {
    db.with(|c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        check_owner_origin(&tx, &input.credential)?;
        let challenge = Sha256::digest(
            format!(
                "pockymoe-add-credential-v1\n{}\n{}\n{}",
                device_id(&tx)?,
                input.credential.credential_id,
                input.credential.public_key
            )
            .as_bytes(),
        );
        verify(&input.credential, &input.assertion, &challenge)?;
        if let Some(assertion) = &input.certificate {
            let bundle = local_bundle(&tx)?.context("missing local certificate")?;
            let base = bundle
                .certificates
                .first()
                .context("empty local certificate bundle")?;
            verify(
                &input.credential,
                assertion,
                &certificate_challenge(&bundle.device_id, &base.identity_key, &base.pairing_id),
            )?;
            append_certificate(
                &tx,
                DeviceCertificate {
                    version: 1,
                    device_id: bundle.device_id,
                    identity_key: base.identity_key.clone(),
                    pairing_id: base.pairing_id.clone(),
                    assertion: assertion.clone(),
                },
            )?;
        }
        store_credential(&tx, &input.credential)?;
        let result = json!({"credentials":credentials(&tx)?});
        tx.commit()?;
        Ok(result)
    })
}
fn remove_credential(db: &Database, id: &str) -> Result<Value> {
    db.with(|c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        if credential(&tx, id)?.is_some()
            && tx.query_row("SELECT count(*) FROM owner_credentials", [], |r| {
                r.get::<_, u64>(0)
            })? <= 1
        {
            return reject(409, "last_owner_credential");
        }
        let time = now();
        collect_revoked_sessions(&tx, time)?;
        let sessions = tx
            .prepare("SELECT session_id,expires_at FROM owner_sessions WHERE credential_id=?1 AND expires_at>?2")?
            .query_map(params![id,time], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (session, expires) in sessions {
            remember_revocation(&tx, &session, expires)?;
        }
        // Explicit deletion protects against re-addition even if FK enforcement is off.
        if let Some(mut bundle) = local_bundle(&tx)? {
            bundle.certificates.retain(|cert| cert.assertion.credential_id != id);
            store_bundle(&tx, &bundle)?;
        }
        tx.execute("DELETE FROM peer_certificates WHERE credential_id=?1", [id])?;
        tx.execute("DELETE FROM owner_sessions WHERE credential_id=?1", [id])?;
        tx.execute("DELETE FROM owner_credentials WHERE credential_id=?1", [id])?;
        let result = json!({"credentials":credentials(&tx)?});
        tx.commit()?;
        Ok(result)
    })
}

fn collect_revoked_sessions(c: &Connection, time: i64) -> Result<()> {
    c.execute(
        "DELETE FROM kv WHERE key GLOB 'ownerRevokedSession:*' AND CAST(value AS INTEGER)<=?1",
        [time],
    )?;
    Ok(())
}
fn remember_revocation(c: &Connection, id: &str, expires: i64) -> Result<()> {
    c.execute(
        "INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET
         value=CAST(MAX(CAST(kv.value AS INTEGER),CAST(excluded.value AS INTEGER)) AS TEXT)",
        params![format!("{REVOKED_SESSION_PREFIX}{id}"), expires.to_string()],
    )?;
    Ok(())
}
fn revoke_session(db: &Database, id: &str, time: i64) -> Result<Value> {
    db.with(|c| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
        collect_revoked_sessions(&tx, time)?;
        let expires: Option<i64> = tx
            .query_row(
                "SELECT expires_at FROM owner_sessions WHERE session_id=?1 AND expires_at>?2",
                params![id, time],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(expires) = expires {
            remember_revocation(&tx, id, expires)?;
        }
        tx.execute("DELETE FROM owner_sessions WHERE session_id=?1", [id])?;
        tx.commit()?;
        Ok(json!({}))
    })
}

pub(crate) async fn pair_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Json(p): Json<Pairing>,
) -> Response {
    let result = (|| {
        pair(
            &state.db,
            &crate::secure_transport::transport(&state)?.identity_key(),
            &p,
            now(),
        )
    })();
    if result.is_ok() {
        crate::ports::refresh_owner_permissions(&state);
    }
    response(result)
}
pub(crate) async fn session_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Json(r): Json<Registration>,
) -> Response {
    response(register(&state.db, &r, now()))
}
pub(crate) async fn trust_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(p): Extension<OwnerPrincipal>,
) -> Response {
    response(trust(&state.db, &p))
}
pub(crate) async fn add_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(_): Extension<OwnerPrincipal>,
    Json(input): Json<AddCredential>,
) -> Response {
    response(add(&state.db, &input))
}
pub(crate) async fn remove_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(_): Extension<OwnerPrincipal>,
    Path(id): Path<String>,
) -> Response {
    let result = remove_credential(&state.db, &id);
    if result.is_ok() {
        crate::ports::refresh_owner_permissions(&state);
    }
    response(result)
}
pub(crate) async fn revoke_endpoint(
    State(state): State<Arc<Supervisor>>,
    Extension(_): Extension<EncryptedOwnerRequest>,
    Extension(_): Extension<OwnerPrincipal>,
    Path(id): Path<String>,
) -> Response {
    response(revoke_session(&state.db, &id, now()))
}

fn local_connection(path: &FsPath) -> Result<Connection> {
    validate_database_path(path)?;
    // No Supervisor process lock, migrations, runtime boot, or network access.
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).context(
        "Open the existing Supervisor database; connect an updated Supervisor once first",
    )?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    Ok(c)
}
/// Local database write authority creates the out-of-band pairing capability.
/// The caller displays the returned URL locally; never send it to a relay or log it.
pub fn create_pairing_offer(
    database: &FsPath,
    web_origin: Option<&str>,
    relay_url: Option<&str>,
) -> Result<String> {
    let c = local_connection(database)?;
    let device = device_id(&c)?;
    let origin = match web_origin {
        Some(s) => s.to_owned(),
        None => {
            let u =
                url::Url::parse(relay_url.context("Configure the relay URL or pass --web-origin")?)
                    .map_err(|_| anyhow::anyhow!("Invalid relay URL"))?;
            format!(
                "https://{}",
                u.host_str().context("Relay URL host missing")?
            )
        }
    };
    let u = url::Url::parse(&origin).map_err(|_| anyhow::anyhow!("Invalid Web origin"))?;
    ensure!(
        matches!(u.scheme(), "https" | "http")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none(),
        "--web-origin must be an HTTP(S) origin without credentials, path, query or fragment"
    );
    let origin = u.origin().ascii_serialization();
    let fingerprint = crate::secure_transport::fingerprint(database)?;
    let mut id = [0u8; 16];
    let mut secret = [0u8; 32];
    OsRng.fill_bytes(&mut id);
    OsRng.fill_bytes(&mut secret);
    let pairing = B64.encode(id);
    let time = now();
    c.execute("INSERT INTO pairing_offers(pairing_id,secret,web_origin,rp_id,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6)",params![pairing,secret.as_slice(),origin,u.host_str().unwrap(),time,time+10*60*1000])?;
    let fragment = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("v", "1")
        .append_pair("device", &device)
        .append_pair("pairing", &pairing)
        .append_pair("secret", &B64.encode(secret))
        .append_pair("identity", &fingerprint)
        .finish();
    Ok(format!("{origin}/pair#{fragment}"))
}
/// Reset requires local write authority; the CLI owns the confirmation prompt.
pub fn reset_owner_trust(database: &FsPath) -> Result<()> {
    let c = local_connection(database)?;
    let tx = Transaction::new_unchecked(&c, TransactionBehavior::Immediate)?;
    // Owner-created mappings cannot become legacy relay-trusted mappings on reset.
    let mappings: Option<String> = tx
        .query_row(
            "SELECT value FROM kv WHERE key='devicePortMappings'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(mappings) = mappings {
        let mut mappings: Vec<Value> = serde_json::from_str(&mappings)?;
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
    tx.execute_batch("DELETE FROM peer_certificates; DELETE FROM owner_port_mappings; DELETE FROM owner_sessions; DELETE FROM owner_credentials; DELETE FROM device_certificates; DELETE FROM pairing_offers; DELETE FROM kv WHERE key GLOB 'ownerRevokedSession:*'; UPDATE device_owner_trust SET strict=0 WHERE singleton=1;")?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "owner_trust_tests.rs"]
pub(crate) mod tests;
