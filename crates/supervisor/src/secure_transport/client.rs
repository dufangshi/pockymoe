//! Peer HTTP uses the browser packets, account certificates when bound, and TOFU otherwise.
use super::{now, pack, random, unpack, Kem, INFO, LIMIT};
use crate::peer_link::{relay_request, PeerError, PeerResponse};
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use anyhow::{anyhow, bail, ensure, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64},
    Engine,
};
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, Deserializable, Kem as KemTrait, OpModeS, Serializable,
};
use p256::ecdsa::{
    signature::{Signer, Verifier},
    Signature, VerifyingKey,
};
use pockymoe_protocol::now_rfc3339;
use pockymoe_runtime::Supervisor;
use rand09::SeedableRng;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

const WIRE_BODY_LIMIT: usize = 8 * 1024 * 1024;
const REFRESH_BEFORE_MS: i64 = 300_000;
const MAX_STREAM_CHUNKS: usize = LIMIT / (1024 * 1024) + 1;
const RESPONSE_PACKET_LIMIT: usize = LIMIT + 64 * 1024 + 4 + 16;

#[derive(Clone)]
struct Descriptor {
    key_id: String,
    public: <Kem as KemTrait>::PublicKey,
    expires: i64,
    clock_offset: i64,
}
impl Descriptor {
    fn timestamp(&self) -> Result<i64> {
        now()
            .checked_add(self.clock_offset)
            .ok_or_else(|| anyhow!("invalid peer clock offset"))
    }
}
#[derive(Default)]
struct Device {
    descriptor: Mutex<Option<Descriptor>>,
    handshake: tokio::sync::Mutex<()>,
}
impl Device {
    fn cached(&self) -> Option<Descriptor> {
        self.descriptor
            .lock()
            .unwrap()
            .as_ref()
            .filter(|d| {
                d.timestamp()
                    .is_ok_and(|time| d.expires.saturating_sub(time) > REFRESH_BEFORE_MS)
            })
            .cloned()
    }
    fn invalidate(&self, key_id: &str) {
        let mut descriptor = self.descriptor.lock().unwrap();
        if descriptor.as_ref().is_some_and(|d| d.key_id == key_id) {
            *descriptor = None;
        }
    }
}
#[derive(Default)]
pub(crate) struct Client {
    devices: Mutex<HashMap<String, Arc<Device>>>,
}
struct Sealed {
    payload: Value,
    response_key: [u8; 32],
    aad: String,
}
struct Reply {
    response: PeerResponse,
    next: Option<String>,
}
impl Client {
    fn device(&self, device_id: &str) -> Arc<Device> {
        self.devices
            .lock()
            .unwrap()
            .entry(device_id.into())
            .or_default()
            .clone()
    }
    pub(crate) fn reset_pin(&self, state: &Supervisor, device_id: &str) -> Result<()> {
        let device = self.device(device_id);
        let mut descriptor = device.descriptor.lock().unwrap();
        state.db.with(|conn| {
            conn.execute(
                "DELETE FROM kv WHERE key=?1",
                [format!("peer:pin:{device_id}")],
            )?;
            Ok(())
        })?;
        *descriptor = None;
        Ok(())
    }
    async fn descriptor(&self, state: &Supervisor, device_id: &str) -> Result<Descriptor> {
        let device = self.device(device_id);
        // Bound callers never authenticate from a cached descriptor: a target
        // may have been reset without rotating its transport identity or key.
        if !crate::owner_trust::bound(state) {
            if let Some(descriptor) = device.cached() {
                return Ok(descriptor);
            }
        }
        let _handshake = device.handshake.lock().await;
        if !crate::owner_trust::bound(state) {
            if let Some(descriptor) = device.cached() {
                return Ok(descriptor);
            }
        }
        let challenge = Uuid::new_v4().to_string();
        let started = now();
        let payload = relay_request(
            state,
            device_id,
            json!({"method":"GET","path":format!("/api/peer/transport/key?challenge={challenge}"),"headers":{}}),
        )
        .await?;
        let received = now();
        let response = plain_response(&payload)?;
        if !(200..300).contains(&response.status) {
            return Err(remote_error(device_id, &response).into());
        }
        let value: Value = serde_json::from_slice(&response.body)?;
        let (descriptor, identity) = verify_descriptor(&value, &challenge, started, received)?;
        verify_target(state, device_id, &value, &identity)?;
        let bound = crate::owner_trust::bound(state);
        // Pin and publish under one lock, so a trust reset also invalidates the cache.
        let mut cached = device.descriptor.lock().unwrap();
        let pin_key = format!("peer:pin:{device_id}");
        if let Some(pin) = state.db.get_kv(&pin_key)? {
            let pin: Value = serde_json::from_str(&pin)?;
            ensure!(pin["identityKey"].is_string(), "invalid peer identity pin");
            if pin["identityKey"] != identity && !bound {
                return Err(PeerError::IdentityChanged(device_id.into()).into());
            }
        }
        if bound || state.db.get_kv(&pin_key)?.is_none() {
            state.db.set_kv(
                &pin_key,
                &json!({"identityKey":identity,"fingerprint":B64.encode(Sha256::digest(B64.decode(&identity)?)),"pinnedAt":now_rfc3339()}).to_string(),
            )?;
        }
        *cached = if bound {
            None
        } else {
            Some(descriptor.clone())
        };
        Ok(descriptor)
    }
    async fn one(
        &self,
        state: &Supervisor,
        device_id: &str,
        method: &str,
        path_and_query: &str,
        content_type: Option<&str>,
        body: &[u8],
    ) -> Result<Reply> {
        let mut refreshed = false;
        let mut registered = false;
        let mut descriptor = self.descriptor(state, device_id).await?;
        loop {
            let mut sealed = seal(&descriptor, method, path_and_query, content_type, body)?;
            sign_peer(state, &mut sealed)?;
            let payload =
                relay_request(state, device_id, std::mem::take(&mut sealed.payload)).await?;
            let reply = if payload["headers"]["x-rcd-encrypted"] == "1" {
                open(&sealed, &payload)?
            } else {
                Reply {
                    response: plain_response(&payload)?,
                    next: None,
                }
            };
            let error = remote_error(device_id, &reply.response);
            if !refreshed
                && matches!(&error, PeerError::Remote { status: 409, code, .. } if code == "transport_reconnect_required")
            {
                self.device(device_id).invalidate(&descriptor.key_id);
                refreshed = true;
                descriptor = self.descriptor(state, device_id).await?;
                continue;
            }
            if !registered
                && crate::owner_trust::bound(state)
                && matches!(&error, PeerError::Remote { status: 401, code, .. } if code == "peer_certificate_required")
            {
                let certificate = crate::owner_trust::device_certificate(state)?
                    .ok_or_else(|| anyhow!("missing local peer device certificate"))?;
                let body = serde_json::to_vec(&certificate)?;
                let mut registration = seal(
                    &descriptor,
                    "POST",
                    "/api/peer/transport/certificate",
                    Some("application/json"),
                    &body,
                )?;
                sign_peer(state, &mut registration)?;
                let payload =
                    relay_request(state, device_id, std::mem::take(&mut registration.payload))
                        .await?;
                // Keep relay/offline errors retryable by the durable outbox, while
                // requiring successful registration replies to be encrypted.
                if payload["headers"]["x-rcd-encrypted"] != "1" {
                    let response = plain_response(&payload)?;
                    let error = remote_error(device_id, &response);
                    if !refreshed
                        && matches!(&error, PeerError::Remote { status: 409, code, .. } if code == "transport_reconnect_required")
                    {
                        self.device(device_id).invalidate(&descriptor.key_id);
                        refreshed = true;
                        descriptor = self.descriptor(state, device_id).await?;
                        continue;
                    }
                    ensure!(
                        !(200..300).contains(&response.status),
                        "peer registration response must be encrypted"
                    );
                    return Err(error.into());
                }
                let response = open(&registration, &payload)?.response;
                if !(200..300).contains(&response.status) {
                    return Err(remote_error(device_id, &response).into());
                }
                registered = true;
                continue;
            }
            if payload["headers"]["x-rcd-encrypted"] != "1" {
                return Err(error.into());
            }
            return Ok(reply);
        }
    }
    pub(crate) async fn request(
        &self,
        state: &Supervisor,
        device_id: &str,
        method: &str,
        path_and_query: &str,
        content_type: Option<&str>,
        body: &[u8],
    ) -> Result<PeerResponse> {
        ensure!(!device_id.is_empty(), "missing peer device id");
        ensure!(
            matches!(method, "GET" | "POST" | "PUT"),
            "invalid peer method"
        );
        let path = path_and_query.split('?').next().unwrap_or("");
        ensure!(
            path.starts_with("/api/peer/") && !path_and_query.contains('#'),
            "invalid peer path"
        );
        let uri: axum::http::Uri = path_and_query.parse()?;
        ensure!(
            uri.scheme().is_none() && uri.authority().is_none(),
            "invalid peer path"
        );
        ensure_body_size(body.len())?;
        let mut reply = self
            .one(state, device_id, method, path_and_query, content_type, body)
            .await?;
        let mut seen = HashSet::new();
        while let Some(next) = reply.next.take() {
            let route = next.split('?').next().unwrap_or("");
            ensure!(
                route.starts_with("/api/peer/transport/stream/")
                    && !next.contains('#')
                    && seen.len() < MAX_STREAM_CHUNKS
                    && seen.insert(next.clone()),
                "invalid peer download continuation"
            );
            let chunk = self.one(state, device_id, "GET", &next, None, b"").await?;
            if !(200..300).contains(&chunk.response.status) {
                return Err(handler_error(&chunk.response).into());
            }
            ensure!(
                chunk.response.body.len() <= LIMIT - reply.response.body.len(),
                "peer response exceeds the 64 MiB limit"
            );
            reply.response.body.extend_from_slice(&chunk.response.body);
            reply.next = chunk.next;
        }
        Ok(reply.response)
    }
}
fn verify_descriptor(
    value: &Value,
    challenge: &str,
    started: i64,
    received: i64,
) -> Result<(Descriptor, String)> {
    ensure!(
        value["version"] == 1
            && value["suite"] == "DHKEM-P256-HKDF-SHA256/AES-256-GCM"
            && value["challenge"] == challenge,
        "invalid peer key descriptor or challenge"
    );
    let text = |key: &str| {
        value[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing peer key {key}"))
    };
    let key_id = text("keyId")?;
    let public = text("publicKey")?;
    let expires = value["expiresAt"]
        .as_i64()
        .ok_or_else(|| anyhow!("missing peer key expiry"))?;
    let server_time = value["serverTime"]
        .as_i64()
        .ok_or_else(|| anyhow!("missing peer server time"))?;
    let identity = VerifyingKey::from_sec1_bytes(&B64.decode(text("identityKey")?)?)?;
    let signature = Signature::from_slice(&B64.decode(text("signature")?)?)?;
    let signed = format!("rcd-key-v1\n{challenge}\n{key_id}\n{expires}\n{server_time}\n{public}");
    identity
        .verify(signed.as_bytes(), &signature)
        .map_err(|_| anyhow!("peer key signature is invalid"))?;
    ensure!(expires > server_time, "peer encryption key expired");
    let midpoint = started.saturating_add(received.saturating_sub(started) / 2);
    let clock_offset = server_time
        .checked_sub(midpoint)
        .ok_or_else(|| anyhow!("invalid peer server time"))?;
    let descriptor = Descriptor {
        key_id: key_id.into(),
        public: <Kem as KemTrait>::PublicKey::from_bytes(&B64.decode(public)?)?,
        expires,
        clock_offset,
    };
    ensure!(
        descriptor.timestamp()? < expires,
        "peer encryption key expired"
    );
    Ok((
        descriptor,
        B64.encode(identity.to_encoded_point(false).as_bytes()),
    ))
}
fn verify_target(state: &Supervisor, device: &str, value: &Value, identity: &str) -> Result<()> {
    if crate::owner_trust::bound(state) {
        crate::owner_trust::verify_peer_certificate(
            state,
            value["certificate"].clone(),
            device,
            Some(identity),
        )
        .map_err(|_| PeerError::TargetUnverified(device.into()))?;
    }
    Ok(())
}
fn sign_peer(state: &Supervisor, sealed: &mut Sealed) -> Result<()> {
    if !crate::owner_trust::bound(state) {
        return Ok(());
    }
    let certificate = crate::owner_trust::device_certificate(state)?
        .ok_or_else(|| anyhow!("missing local peer device certificate"))?;
    let ciphertext = STANDARD.decode(sealed.payload["body"].as_str().unwrap())?;
    let input =
        crate::owner_trust::peer_signing_input(&certificate.device_id, &sealed.aad, &ciphertext);
    let signature: Signature = super::transport(state)?.identity.sign(input.as_bytes());
    sealed.payload["headers"]["x-rcd-peer-device"] = json!(certificate.device_id);
    sealed.payload["headers"]["x-rcd-peer-signature"] = json!(B64.encode(signature.to_bytes()));
    Ok(())
}
fn ensure_body_size(size: usize) -> Result<()> {
    if size > WIRE_BODY_LIMIT / 4 * 3 {
        return Err(payload_too_large().into());
    }
    Ok(())
}
fn payload_too_large() -> PeerError {
    PeerError::Remote {
        status: 413,
        code: "payload_too_large".into(),
        message: "Encrypted peer request exceeds the 8 MiB relay body limit.".into(),
    }
}
fn seal(
    descriptor: &Descriptor,
    method: &str,
    path_and_query: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Sealed> {
    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, String::new()), |(path, query)| {
            (path, format!("?{query}"))
        });
    let request_id = format!("{}.{}", Uuid::new_v4(), descriptor.timestamp()?);
    let key_id = &descriptor.key_id;
    let aad = format!("rcd-http-v1\n{key_id}\n{request_id}\n{method}\n{path}\n");
    let mut headers = Map::new();
    if let Some(content_type) = content_type {
        headers.insert("content-type".into(), json!(content_type));
    }
    let clear = pack(&json!({"query":query,"headers":headers}), body)?;
    if (clear.len() + 16).div_ceil(3) * 4 > WIRE_BODY_LIMIT {
        return Err(payload_too_large().into());
    }
    let mut rng = rand09::rngs::StdRng::from_seed(random());
    let (enc, mut sender) = hpke::setup_sender::<AesGcm256, HkdfSha256, Kem, _>(
        &OpModeS::Base,
        &descriptor.public,
        INFO,
        &mut rng,
    )?;
    let sealed = sender.seal(&clear, aad.as_bytes())?;
    let mut response_key = [0u8; 32];
    sender.export(b"remote-codex/http-response/v1", &mut response_key)?;
    Ok(Sealed {
        payload: json!({"method":method,"path":path,"headers":{"content-type":"application/octet-stream","x-rcd-key":key_id,"x-rcd-request":request_id,"x-rcd-enc":B64.encode(enc.to_bytes())},"body":STANDARD.encode(sealed),"bodyEncoding":"base64"}),
        response_key,
        aad,
    })
}
fn decode_body(payload: &Value, limit: usize) -> Result<Vec<u8>> {
    let body = payload["body"]
        .as_str()
        .ok_or_else(|| anyhow!("invalid peer response body"))?;
    let bytes = if payload["bodyEncoding"] == "base64" {
        ensure!(
            body.len() <= limit.div_ceil(3) * 4,
            "peer response exceeds the size limit"
        );
        STANDARD.decode(body)?
    } else {
        ensure!(body.len() <= limit, "peer response exceeds the size limit");
        body.as_bytes().to_vec()
    };
    ensure!(bytes.len() <= limit, "peer response exceeds the size limit");
    Ok(bytes)
}
fn status(value: &Value) -> Result<u16> {
    value
        .as_u64()
        .filter(|s| (100..600).contains(s))
        .map(|s| s as u16)
        .ok_or_else(|| anyhow!("invalid peer response status"))
}
fn response_headers(value: &Value) -> Result<Map<String, Value>> {
    let headers = value
        .as_object()
        .ok_or_else(|| anyhow!("invalid peer response headers"))?;
    ensure!(
        headers.values().all(Value::is_string),
        "invalid peer response headers"
    );
    Ok(headers.clone())
}
fn plain_response(payload: &Value) -> Result<PeerResponse> {
    Ok(PeerResponse {
        status: status(&payload["statusCode"])?,
        headers: response_headers(&payload["headers"])?,
        body: decode_body(payload, LIMIT)?,
    })
}
pub(crate) fn relay_error(device_id: &str, payload: &Value) -> Result<PeerError> {
    Ok(remote_error(device_id, &plain_response(payload)?))
}
fn remote_error(device_id: &str, response: &PeerResponse) -> PeerError {
    match response.status {
        503 => PeerError::Offline(device_id.into()),
        504 => PeerError::Timeout(device_id.into()),
        _ => handler_error(response),
    }
}
fn handler_error(response: &PeerResponse) -> PeerError {
    PeerError::from_reply(response.status, &response.body, "Peer request failed.")
}
fn open(sealed: &Sealed, payload: &Value) -> Result<Reply> {
    let ciphertext = decode_body(payload, RESPONSE_PACKET_LIMIT)?;
    let aad = format!("{}\nresponse", sealed.aad);
    let clear = Aes256Gcm::new_from_slice(&sealed.response_key)
        .map_err(|_| anyhow!("invalid peer response key"))?
        .decrypt(
            Nonce::from_slice(&[0u8; 12]),
            Payload {
                msg: &ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow!("peer response authentication failed"))?;
    let (metadata, body) = unpack(&clear)?;
    ensure!(
        body.len() <= LIMIT,
        "peer response exceeds the 64 MiB limit"
    );
    let next = match &metadata["streamNext"] {
        Value::Null => None,
        Value::String(next) => Some(next.clone()),
        _ => bail!("invalid peer download continuation"),
    };
    Ok(Reply {
        response: PeerResponse {
            status: status(&metadata["status"])?,
            headers: response_headers(&metadata["headers"])?,
            body: body.into(),
        },
        next,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::{PeerCaller, TrustedRelayForward},
        peer_link::{self, tests::connected},
        tunnel::{forward_local, tests::state_with_relay_url},
    };
    use axum::body::{Body, Bytes};
    use p256::ecdsa::signature::Signer;
    use tower::ServiceExt;

    #[derive(Default)]
    struct Service {
        forward: bool,
        bad_signature: bool,
        bad_peer_signature: bool,
        rotate: usize,
        plain_error: Option<u16>,
        registration_error: Option<u16>,
        download_size: Option<usize>,
        stream_error: Option<u16>,
        status: Option<u16>,
        body: Option<Vec<u8>>,
        continuation: Option<String>,
    }
    fn peer() -> PeerCaller {
        PeerCaller {
            device_id: "caller".into(),
            device_name: "Caller".into(),
            user_id: "owner".into(),
            shared_control: None,
        }
    }
    fn serve(
        caller: &Supervisor,
        target: Arc<Supervisor>,
        mut service: Service,
    ) -> (tokio::task::JoinHandle<()>, Arc<Mutex<Vec<Value>>>) {
        let (connection, mut outbound) = connected(caller, "caller");
        let frames = Arc::new(Mutex::new(Vec::new()));
        let recorded = frames.clone();
        let transport = crate::secure_transport::transport(&target).unwrap();
        let task = tokio::spawn(async move {
            while let Some(frame) = outbound.recv().await {
                assert_eq!(frame["type"], "peer.request");
                assert_eq!(frame["targetDeviceId"], "target");
                let mut payload = frame["payload"].clone();
                if service.bad_peer_signature
                    && payload["path"] != "/api/peer/transport/certificate"
                {
                    payload["headers"]["x-rcd-peer-signature"] = json!(B64.encode([0; 64]));
                }
                recorded.lock().unwrap().push(payload.clone());
                let plain_error = if payload["path"] == "/api/peer/transport/certificate" {
                    service.registration_error.or(service.plain_error)
                } else {
                    service.plain_error
                };
                let response = if let Some(status) = plain_error {
                    json!({"statusCode":status,"headers":{"content-type":"application/json"},"body":json!({"code":"relay_error","message":"relay rejected the request"}).to_string()})
                } else if payload["path"]
                    .as_str()
                    .unwrap()
                    .starts_with("/api/peer/transport/key")
                {
                    let mut response = forward_local(&target, payload, Some(peer())).await;
                    if service.bad_signature {
                        let mut descriptor: Value =
                            serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
                        descriptor["signature"] = json!(B64.encode([0u8; 64]));
                        response["body"] = json!(descriptor.to_string());
                    }
                    response
                } else if service.rotate > 0 {
                    service.rotate -= 1;
                    for key in transport.keys.lock().unwrap().values_mut() {
                        key.expires = now() - 61_000;
                    }
                    forward_local(&target, payload, Some(peer())).await
                } else if service.forward
                    || !transport.has_key(payload["headers"]["x-rcd-key"].as_str().unwrap())
                {
                    forward_local(&target, payload, Some(peer())).await
                } else {
                    let opened = transport.open(&payload).unwrap();
                    let path = opened.request["path"].as_str().unwrap();
                    let response = if path.starts_with("/api/peer/transport/stream/") {
                        if let Some(status) = service.stream_error {
                            json!({"statusCode":status,"headers":{"content-type":"application/json"},"body":r#"{"code":"download_failed","message":"Target download failed"}"#})
                        } else {
                            transport
                                .streams
                                .read(path, &super::super::StreamPrincipal::Unauthenticated)
                                .await
                                .unwrap()
                        }
                    } else if let Some(size) = service.download_size {
                        let chunk = Bytes::from(
                            (0..1024 * 1024)
                                .map(|i| (i % 251) as u8)
                                .collect::<Vec<_>>(),
                        );
                        let chunks = (0..size.div_ceil(chunk.len())).map(move |i| {
                            let len = (size - i * chunk.len()).min(chunk.len());
                            Ok::<_, std::io::Error>(chunk.slice(..len))
                        });
                        let (body, next) = transport
                            .streams
                            .begin(
                                path,
                                Body::from_stream(futures_util::stream::iter(chunks)),
                                super::super::StreamPrincipal::Unauthenticated,
                            )
                            .await
                            .unwrap();
                        json!({"statusCode":206,"headers":{"content-type":"application/x-peer-test","content-range":"bytes 0-2/*"},"body":STANDARD.encode(body),"bodyEncoding":"base64","streamNext":next})
                    } else {
                        let body = service.body.clone().unwrap_or_else(|| {
                            crate::tunnel::decode_relay_request_body(&opened.request)
                                .unwrap()
                                .unwrap()
                        });
                        json!({"statusCode":service.status.unwrap_or(201),"headers":{"content-type":opened.request["headers"]["content-type"].as_str().unwrap_or("application/octet-stream"),"x-seen-path":path},"body":STANDARD.encode(body),"bodyEncoding":"base64","streamNext":service.continuation})
                    };
                    opened.response(response).unwrap()
                };
                connection.receive(&json!({"type":"peer.response","requestId":frame["requestId"],"payload":response}));
            }
        });
        (task, frames)
    }
    async fn stop(task: tokio::task::JoinHandle<()>) {
        task.abort();
        let _ = task.await;
    }
    async fn call(state: &Supervisor) -> Result<PeerResponse> {
        peer_link::request(
            state,
            "target",
            "POST",
            "/api/peer/cli",
            Some("application/json"),
            br#"{"operation":"info"}"#.to_vec(),
        )
        .await
    }
    #[tokio::test]
    async fn peer_client_roundtrip_through_forward_local_and_router() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        let expected = crate::http::router(target.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/peer/cli")
                    .header("content-type", "application/json")
                    .extension(TrustedRelayForward)
                    .extension(peer())
                    .body(Body::from(br#"{"operation":"info"}"#.to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let expected_status = expected.status().as_u16();
        let expected_body = axum::body::to_bytes(expected.into_body(), 4096)
            .await
            .unwrap();
        let (task, frames) = serve(
            &caller,
            target.clone(),
            Service {
                forward: true,
                ..Service::default()
            },
        );
        let response = call(&caller).await.unwrap();
        assert_eq!(response.status, expected_status);
        assert_eq!(response.body, expected_body);
        assert_eq!(response.headers["content-type"], "application/json");
        let pin: Value =
            serde_json::from_str(&caller.db.get_kv("peer:pin:target").unwrap().unwrap()).unwrap();
        assert_eq!(
            pin["fingerprint"],
            crate::secure_transport::fingerprint(&target.config.database_url).unwrap()
        );
        assert!(pin["pinnedAt"].is_string());
        assert_eq!(frames.lock().unwrap().len(), 2);
        stop(task).await;
    }
    #[tokio::test]
    async fn peer_client_preserves_binary_query_headers_and_cached_keys() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        let (task, frames) = serve(&caller, target, Service::default());
        let bytes = (0..4 * 1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        let response = peer_link::request(
            &caller,
            "target",
            "PUT",
            "/api/peer/files/uploads/test?offset=19&name=private-name",
            Some("application/x-test"),
            bytes.clone(),
        )
        .await
        .unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.body, bytes);
        assert_eq!(response.headers["content-type"], "application/x-test");
        assert_eq!(
            response.headers["x-seen-path"],
            "/api/peer/files/uploads/test?offset=19&name=private-name"
        );
        call(&caller).await.unwrap();
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1]["path"], "/api/peer/files/uploads/test");
        assert!(!frames[1].to_string().contains("private-name"));
        assert!(!frames[1].to_string().contains("application/x-test"));
        drop(frames);
        stop(task).await;
    }
    #[tokio::test]
    async fn peer_client_rejects_bad_signature_without_pinning() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        let (task, frames) = serve(
            &caller,
            target,
            Service {
                bad_signature: true,
                ..Service::default()
            },
        );
        assert!(call(&caller)
            .await
            .unwrap_err()
            .to_string()
            .contains("signature"));
        assert!(caller.db.get_kv("peer:pin:target").unwrap().is_none());
        assert_eq!(frames.lock().unwrap().len(), 1);
        stop(task).await;
    }
    #[tokio::test]
    async fn peer_client_identity_change_requires_reset_and_is_database_scoped() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_other_dir, other) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        let (_replacement_dir, replacement) = state_with_relay_url("http://127.0.0.1:1");
        for state in [&caller, &other, &target, &replacement] {
            state.set_peer_access(true).unwrap();
        }
        let (task, _) = serve(&caller, target, Service::default());
        call(&caller).await.unwrap();
        let pin = caller.db.get_kv("peer:pin:target").unwrap();
        stop(task).await;
        let (task, frames) = serve(&caller, replacement.clone(), Service::default());
        assert!(
            matches!(call(&caller).await.unwrap_err().downcast_ref(), Some(PeerError::IdentityChanged(id)) if id == "target")
        );
        assert_eq!(caller.db.get_kv("peer:pin:target").unwrap(), pin);
        assert_eq!(frames.lock().unwrap().len(), 2);
        let (other_task, _) = serve(&other, replacement, Service::default());
        call(&other).await.unwrap();
        assert_ne!(other.db.get_kv("peer:pin:target").unwrap(), pin);
        peer_link::reset_pin(&caller, "target").unwrap();
        assert!(caller.db.get_kv("peer:pin:target").unwrap().is_none());
        call(&caller).await.unwrap();
        let caller_pin: Value =
            serde_json::from_str(&caller.db.get_kv("peer:pin:target").unwrap().unwrap()).unwrap();
        let other_pin: Value =
            serde_json::from_str(&other.db.get_kv("peer:pin:target").unwrap().unwrap()).unwrap();
        assert_eq!(caller_pin["identityKey"], other_pin["identityKey"]);
        stop(task).await;
        stop(other_task).await;
    }
    #[tokio::test]
    async fn peer_client_refreshes_409_once_and_keeps_identity_pin() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        let (task, frames) = serve(
            &caller,
            target.clone(),
            Service {
                rotate: 1,
                ..Service::default()
            },
        );
        assert_eq!(call(&caller).await.unwrap().status, 201);
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 4);
        assert_ne!(
            frames[1]["headers"]["x-rcd-key"],
            frames[3]["headers"]["x-rcd-key"]
        );
        assert_ne!(
            frames[1]["headers"]["x-rcd-request"],
            frames[3]["headers"]["x-rcd-request"]
        );
        drop(frames);
        stop(task).await;
        peer_link::reset_pin(&caller, "target").unwrap();
        let (task, frames) = serve(
            &caller,
            target,
            Service {
                rotate: 2,
                ..Service::default()
            },
        );
        assert!(
            matches!(call(&caller).await.unwrap_err().downcast_ref(), Some(PeerError::Remote { status:409, code, .. }) if code == "transport_reconnect_required")
        );
        assert_eq!(frames.lock().unwrap().len(), 4);
        stop(task).await;
    }
    #[tokio::test]
    async fn peer_client_stream_next_is_encrypted_and_preserves_first_response() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        let size = 2 * 1024 * 1024 + 19;
        let (task, frames) = serve(
            &caller,
            target,
            Service {
                download_size: Some(size),
                ..Service::default()
            },
        );
        let response = peer_link::request(
            &caller,
            "target",
            "GET",
            "/api/peer/files/read?workspaceId=hidden&path=secret.txt",
            None,
            vec![],
        )
        .await
        .unwrap();
        assert_eq!(response.status, 206);
        assert_eq!(response.headers["content-type"], "application/x-peer-test");
        assert_eq!(response.body.len(), size);
        for (i, byte) in response.body.iter().enumerate() {
            assert_eq!(*byte, (i % (1024 * 1024) % 251) as u8);
        }
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 4);
        for frame in &frames[1..] {
            assert!(frame["headers"]["x-rcd-key"].is_string());
            assert!(!frame.to_string().contains("secret.txt"));
            assert!(!frame["path"].as_str().unwrap().contains('?'));
        }
        assert!(frames[2]["path"]
            .as_str()
            .unwrap()
            .starts_with("/api/peer/transport/stream/"));
        drop(frames);
        stop(task).await;
    }
    #[tokio::test]
    async fn peer_client_stream_rejects_aggregate_over_64_mib_and_foreign_paths() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        let (task, _) = serve(
            &caller,
            target.clone(),
            Service {
                download_size: Some(LIMIT + 1),
                ..Service::default()
            },
        );
        assert!(call(&caller)
            .await
            .unwrap_err()
            .to_string()
            .contains("64 MiB"));
        stop(task).await;
        let (task, frames) = serve(
            &caller,
            target,
            Service {
                continuation: Some("/api/config/runtime".into()),
                ..Service::default()
            },
        );
        assert!(call(&caller)
            .await
            .unwrap_err()
            .to_string()
            .contains("continuation"));
        assert_eq!(frames.lock().unwrap().len(), 1);
        stop(task).await;
    }
    #[tokio::test]
    async fn peer_client_stream_target_failure_is_remote_and_not_retryable() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        let (task, _) = serve(
            &caller,
            target,
            Service {
                download_size: Some(1024 * 1024 + 1),
                stream_error: Some(503),
                ..Service::default()
            },
        );
        let error = call(&caller).await.unwrap_err();
        let error = error.downcast_ref::<PeerError>().unwrap();
        assert!(
            matches!(error, PeerError::Remote { status:503, code, .. } if code == "download_failed")
        );
        assert!(!error.retryable());
        stop(task).await;
    }
    #[tokio::test]
    async fn peer_client_relay_errors_and_remote_json_errors_have_contract_types() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        caller.set_peer_access(true).unwrap();
        target.set_peer_access(true).unwrap();
        for status in [404, 403, 413, 429, 503, 504] {
            let (task, _) = serve(
                &caller,
                target.clone(),
                Service {
                    plain_error: Some(status),
                    ..Service::default()
                },
            );
            let error = call(&caller).await.unwrap_err();
            match (status, error.downcast_ref::<PeerError>().unwrap()) {
                (503, PeerError::Offline(id)) | (504, PeerError::Timeout(id)) => {
                    assert_eq!(id, "target")
                }
                (
                    _,
                    PeerError::Remote {
                        status: s,
                        code,
                        message,
                    },
                ) => {
                    assert_eq!(*s, status);
                    assert_eq!(code, "relay_error");
                    assert_eq!(message, "relay rejected the request");
                }
                _ => panic!("unexpected error: {error}"),
            }
            stop(task).await;
        }
        let (task, _) = serve(
            &caller,
            target,
            Service {
                status: Some(422),
                body: Some(br#"{"code":"invalid","message":"remote rejection"}"#.to_vec()),
                ..Service::default()
            },
        );
        assert!(
            matches!(peer_link::request_json(&caller, "target", "/api/peer/cli", &json!({})).await.unwrap_err().downcast_ref(), Some(PeerError::Remote { status:422, code, message }) if code == "invalid" && message == "remote rejection")
        );
        stop(task).await;
        assert!(matches!(
            call(&caller).await.unwrap_err().downcast_ref(),
            Some(PeerError::RelayUnavailable)
        ));
    }
    #[test]
    fn peer_client_descriptor_binds_challenge_expiry_and_remote_clock() {
        let (_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        let transport = crate::secure_transport::transport(&target).unwrap();
        let challenge = "fresh-challenge";
        let original = transport.descriptor(challenge).unwrap();
        assert!(verify_descriptor(&original, "wrong-challenge", now(), now()).is_err());
        for skew in [-86_400_000i64, 86_400_000] {
            let mut value = original.clone();
            let time = now();
            value["serverTime"] = json!(time + skew);
            value["expiresAt"] = json!(time + skew + 3_600_000);
            let signed = format!(
                "rcd-key-v1\n{challenge}\n{}\n{}\n{}\n{}",
                value["keyId"].as_str().unwrap(),
                value["expiresAt"],
                value["serverTime"],
                value["publicKey"].as_str().unwrap()
            );
            let signature: Signature = transport.identity.sign(signed.as_bytes());
            value["signature"] = json!(B64.encode(signature.to_bytes()));
            let (descriptor, _) = verify_descriptor(&value, challenge, time, time).unwrap();
            assert_eq!(descriptor.clock_offset, skew);
            let device = Device::default();
            *device.descriptor.lock().unwrap() = Some(descriptor.clone());
            assert!(device.cached().is_some());
            let sealed = seal(
                &descriptor,
                "GET",
                "/api/peer/files/read?offset=1",
                None,
                b"",
            )
            .unwrap();
            let request_time: i64 = sealed.payload["headers"]["x-rcd-request"]
                .as_str()
                .unwrap()
                .rsplit_once('.')
                .unwrap()
                .1
                .parse()
                .unwrap();
            assert!((time + skew).abs_diff(request_time) < 1000);
            value["expiresAt"] = value["serverTime"].clone();
            let signed = format!(
                "rcd-key-v1\n{challenge}\n{}\n{}\n{}\n{}",
                value["keyId"].as_str().unwrap(),
                value["expiresAt"],
                value["serverTime"],
                value["publicKey"].as_str().unwrap()
            );
            let signature: Signature = transport.identity.sign(signed.as_bytes());
            value["signature"] = json!(B64.encode(signature.to_bytes()));
            assert!(verify_descriptor(&value, challenge, time, time)
                .err()
                .unwrap()
                .to_string()
                .contains("expired"));
        }
    }
    #[test]
    fn peer_client_authenticates_responses_and_bounds_wire_bodies() {
        let (_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        let transport = crate::secure_transport::transport(&target).unwrap();
        let value = transport.descriptor("challenge").unwrap();
        let (descriptor, _) = verify_descriptor(&value, "challenge", now(), now()).unwrap();
        let sealed = seal(&descriptor, "GET", "/api/peer/files/read", None, b"").unwrap();
        let response = transport
            .open(&sealed.payload)
            .unwrap()
            .response(json!({"statusCode":200,"headers":{},"body":"secret"}))
            .unwrap();
        assert_eq!(open(&sealed, &response).unwrap().response.body, b"secret");
        let wrong = seal(&descriptor, "GET", "/api/peer/files/read", None, b"").unwrap();
        assert!(open(&wrong, &response).is_err());
        let mut tampered = response.clone();
        let mut ciphertext = STANDARD.decode(tampered["body"].as_str().unwrap()).unwrap();
        ciphertext[0] ^= 1;
        tampered["body"] = json!(STANDARD.encode(ciphertext));
        assert!(open(&sealed, &tampered).is_err());
        let error = seal(
            &descriptor,
            "PUT",
            "/api/peer/files/uploads/test",
            None,
            &vec![0; WIRE_BODY_LIMIT / 4 * 3],
        )
        .err()
        .unwrap();
        assert!(matches!(
            error.downcast_ref(),
            Some(PeerError::Remote { status: 413, .. })
        ));
    }
    #[tokio::test]
    async fn bound_client_verifies_current_target_replaces_tofu_and_refuses_revocation() {
        use crate::owner_trust::{self, tests::bound_peer};
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        for state in [&caller, &target] {
            state.set_peer_access(true).unwrap();
        }
        bound_peer(&caller, "caller");
        let (task, _) = serve(
            &caller,
            target.clone(),
            Service {
                forward: true,
                ..Default::default()
            },
        );
        let error = call(&caller).await.unwrap_err();
        assert!(
            matches!(error.downcast_ref(),Some(PeerError::TargetUnverified(id)) if id == "target")
        );
        assert!(error
            .to_string()
            .contains("bind the target device to the same account key"));
        bound_peer(&target, "target");
        caller
            .db
            .set_kv(
                "peer:pin:target",
                &json!({"identityKey":"old-tofu-key"}).to_string(),
            )
            .unwrap();
        assert_eq!(call(&caller).await.unwrap().status, 200);
        let pin: Value =
            serde_json::from_str(&caller.db.get_kv("peer:pin:target").unwrap().unwrap()).unwrap();
        assert_eq!(
            pin["identityKey"],
            super::super::transport(&target).unwrap().identity_key()
        );
        let mut revocation: owner_trust::Revocation =
            serde_json::from_value(owner_trust::tests::vectors()["revocations"]["device"].clone())
                .unwrap();
        revocation.subject_id = "target".into();
        use p256::ecdsa::{signature::Signer, Signature};
        let sig: Signature = owner_trust::tests::account_key()
            .sign(owner_trust::revocation_signing_input(&revocation).as_bytes());
        revocation.signature = B64.encode(sig.to_bytes());
        owner_trust::apply_revocations(&caller, &[revocation]).unwrap();
        assert!(matches!(
            call(&caller).await.unwrap_err().downcast_ref(),
            Some(PeerError::TargetUnverified(_))
        ));
        stop(task).await;
    }
    #[tokio::test]
    async fn strict_target_refuses_unbound_caller_and_certificate_retry_is_once() {
        use crate::owner_trust::tests::bound_peer;
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        for state in [&caller, &target] {
            state.set_peer_access(true).unwrap();
        }
        bound_peer(&target, "target");
        let (task, frames) = serve(
            &caller,
            target.clone(),
            Service {
                forward: true,
                ..Default::default()
            },
        );
        let response = call(&caller).await.unwrap();
        assert_eq!(response.status, 401);
        assert_eq!(
            serde_json::from_slice::<Value>(&response.body).unwrap()["code"],
            "peer_certificate_required"
        );
        assert_eq!(frames.lock().unwrap().len(), 2); // unbound callers do not register
        stop(task).await;
        bound_peer(&caller, "caller");
        peer_link::reset_pin(&caller, "target").unwrap();
        let (task, frames) = serve(
            &caller,
            target,
            Service {
                forward: true,
                bad_peer_signature: true,
                ..Default::default()
            },
        );
        assert_eq!(call(&caller).await.unwrap().status, 401);
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[2]["path"], "/api/peer/transport/certificate");
        assert_ne!(
            frames[1]["headers"]["x-rcd-request"],
            frames[3]["headers"]["x-rcd-request"]
        );
        drop(frames);
        stop(task).await;
    }
    #[tokio::test]
    async fn offline_during_certificate_registration_remains_retryable() {
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        for (state, id) in [(&caller, "caller"), (&target, "target")] {
            state.set_peer_access(true).unwrap();
            crate::owner_trust::tests::bound_peer(state, id);
        }
        let (task, frames) = serve(
            &caller,
            target,
            Service {
                forward: true,
                registration_error: Some(503),
                ..Default::default()
            },
        );
        let error = call(&caller).await.unwrap_err();
        assert!(matches!(error.downcast_ref(),Some(PeerError::Offline(id)) if id == "target"));
        assert!(error.downcast_ref::<PeerError>().unwrap().retryable());
        assert_eq!(frames.lock().unwrap().len(), 3);
        stop(task).await;
    }
    #[tokio::test]
    async fn bound_client_refuses_reset_target_from_current_descriptor_even_with_cached_key() {
        use crate::owner_trust::{self, tests::bound_peer};
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        for (state, id) in [(&caller, "caller"), (&target, "target")] {
            state.set_peer_access(true).unwrap();
            bound_peer(state, id);
        }
        let (task, frames) = serve(
            &caller,
            target.clone(),
            Service {
                forward: true,
                ..Default::default()
            },
        );
        let client = Client::default();
        assert_eq!(
            client
                .request(
                    &caller,
                    "target",
                    "POST",
                    "/api/peer/cli",
                    Some("application/json"),
                    br#"{"operation":"info"}"#,
                )
                .await
                .unwrap()
                .status,
            200
        );
        let transport = super::super::transport(&target).unwrap();
        let before = transport.descriptor("old-handshake").unwrap();
        let (stale, _) = verify_descriptor(&before, "old-handshake", now(), now()).unwrap();
        // Populate even an otherwise usable cache to ensure bound callers bypass it.
        *client.device("target").descriptor.lock().unwrap() = Some(stale);
        let previous_pin = caller.db.get_kv("peer:pin:target").unwrap();
        owner_trust::reset_account_trust(&target.config.database_url).unwrap();
        let after = transport.descriptor("new-handshake").unwrap();
        assert_eq!(before["identityKey"], after["identityKey"]);
        assert_eq!(before["keyId"], after["keyId"]);
        let count = frames.lock().unwrap().len();
        assert!(
            matches!(client.request(&caller,"target","POST","/api/peer/cli",Some("application/json"),br#"{"operation":"info"}"#).await.unwrap_err().downcast_ref(),Some(PeerError::TargetUnverified(id)) if id == "target")
        );
        let frames = frames.lock().unwrap();
        assert_eq!(frames.len(), count + 1); // only a new key descriptor, no encrypted action
        assert!(frames.last().unwrap()["path"]
            .as_str()
            .unwrap()
            .starts_with("/api/peer/transport/key?challenge="));
        drop(frames);
        assert_eq!(caller.db.get_kv("peer:pin:target").unwrap(), previous_pin);
        stop(task).await;
    }

    #[tokio::test]
    async fn bound_client_refuses_other_account_wrong_device_identity_and_signature() {
        use crate::owner_trust::{
            self,
            tests::{bound_peer, bound_peer_with_account},
        };
        use p256::ecdsa::{signature::Signer, Signature};
        let (_caller_dir, caller) = state_with_relay_url("http://127.0.0.1:1");
        let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
        for state in [&caller, &target] {
            state.set_peer_access(true).unwrap();
        }
        bound_peer(&caller, "caller");
        let other = p256::ecdsa::SigningKey::random(&mut rand::rngs::OsRng);
        bound_peer_with_account(&target, "target", &other);
        let (task, frames) = serve(
            &caller,
            target.clone(),
            Service {
                forward: true,
                ..Default::default()
            },
        );
        assert!(matches!(
            call(&caller).await.unwrap_err().downcast_ref(),
            Some(PeerError::TargetUnverified(_))
        ));
        owner_trust::reset_account_trust(&target.config.database_url).unwrap();
        let certificate = bound_peer(&target, "target");
        for invalid in [
            {
                let mut c = certificate.clone();
                c.device_id = "wrong-device".into();
                let signature: Signature = owner_trust::tests::account_key()
                    .sign(owner_trust::device_signing_input(&c).as_bytes());
                c.signature = B64.encode(signature.to_bytes());
                c
            },
            {
                let mut c = certificate.clone();
                c.identity_key = owner_trust::tests::vectors()["deviceCertificate"]["certificate"]
                    ["identityKey"]
                    .as_str()
                    .unwrap()
                    .into();
                let signature: Signature = owner_trust::tests::account_key()
                    .sign(owner_trust::device_signing_input(&c).as_bytes());
                c.signature = B64.encode(signature.to_bytes());
                c
            },
            {
                let mut c = certificate.clone();
                c.signature = B64.encode([0; 64]);
                c
            },
        ] {
            target
                .db
                .with(|c| {
                    c.execute(
                        "UPDATE device_certificates SET certificate_json=?1",
                        [serde_json::to_string(&invalid)?],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(matches!(
                call(&caller).await.unwrap_err().downcast_ref(),
                Some(PeerError::TargetUnverified(_))
            ));
        }
        assert_eq!(frames.lock().unwrap().len(), 4);
        assert!(caller.db.get_kv("peer:pin:target").unwrap().is_none());
        stop(task).await;
    }
}
