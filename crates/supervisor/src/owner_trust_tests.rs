use super::*;
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::{DecodePrivateKey, EncodePublicKey},
};
use rand::rngs::OsRng;
use tower::ServiceExt;

fn remote_bind_input() -> Value {
    json!({"accountPublicKey":account(&account_key()).account_public_key,"enrollSecret":B64.encode([59;32]),"webOrigin":"https://web.test"})
}
#[tokio::test]
async fn remote_bind_enrolls_and_preserves_tombstones_while_deleting_keyless_publications() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let previous = live_owner(&state);
    apply_revocations(&state, &[browser_revocation(&previous)]).unwrap();
    reset_account_trust(&state.config.database_url).unwrap();
    let policy = json!({"threadId":"unused","turnIds":[],"afterOrdinal":0,"theme":"dark","createdAt":"unused"});
    state
        .db
        .set_kv("publication:legacy", &policy.to_string())
        .unwrap();
    let mut sealed = policy;
    sealed["encryptionKey"] = json!(B64.encode([0; 32]));
    state
        .db
        .set_kv("publication:encrypted", &sealed.to_string())
        .unwrap();
    let (status, body) = crate::grants::tests::request(
        &state,
        None,
        &session_key(),
        "POST",
        "/api/transport/bind",
        remote_bind_input(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({"bound":true,"deviceId":"test-device","accountKeyId":previous.account_key_id})
    );
    assert!(strict(&state));
    assert!(state.db.get_kv("publication:legacy").unwrap().is_none());
    assert!(state.db.get_kv("publication:encrypted").unwrap().is_some());
    assert_eq!(
        binding_status(&state.config.database_url).unwrap()["webOrigin"],
        "https://web.test"
    );
    assert!(state
        .db
        .with(|c| revoked(c, &previous.account_key_id, "browser", &previous.browser_id))
        .unwrap());
    let (status, enrollment) = crate::grants::tests::request(
        &state,
        None,
        &session_key(),
        "GET",
        "/api/transport/enrollment",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let identity = crate::secure_transport::transport(&state)
        .unwrap()
        .identity_key();
    assert_eq!(
        enrollment["mac"],
        enrollment_mac(
            &[59; 32],
            "test-device",
            &identity,
            &previous.account_key_id
        )
        .unwrap()
    );
    let key = SigningKey::random(&mut OsRng);
    let certificate = browser_certificate(&account_key(), &key);
    assert_eq!(
        crate::grants::tests::request(
            &state,
            None,
            &key,
            "POST",
            "/api/transport/browser",
            serde_json::to_value(&certificate).unwrap()
        )
        .await
        .0,
        200
    );
    let owner = BrowserPrincipal {
        browser_id: certificate.browser_id,
        account_key_id: certificate.account_key_id,
        grantee_user_id: None,
    };
    let mut device = DeviceCertificate {
        version: 1,
        account_key_id: owner.account_key_id.clone(),
        device_id: "test-device".into(),
        identity_key: identity,
        created_at: now(),
        signature: String::new(),
    };
    let signature: Signature = account_key().sign(device_signing_input(&device).as_bytes());
    device.signature = B64.encode(signature.to_bytes());
    assert_eq!(
        crate::grants::tests::request(
            &state,
            Some(&owner),
            &key,
            "PUT",
            "/api/transport/device-certificate",
            serde_json::to_value(&device).unwrap()
        )
        .await
        .0,
        200
    );
    assert_eq!(
        device_certificate(&state).unwrap().unwrap().signature,
        device.signature
    );
    assert_eq!(
        crate::grants::tests::request(
            &state,
            None,
            &key,
            "GET",
            "/api/transport/enrollment",
            Value::Null
        )
        .await,
        (404, json!({"code":"enrollment_missing"}))
    );
    assert_eq!(
        crate::grants::tests::request(&state, None, &key, "GET", "/api/threads", Value::Null).await,
        (401, json!({"code":"browser_registration_required"}))
    );
}
#[tokio::test]
async fn remote_bind_refuses_bound_owner_unsigned_and_grantee_requests() {
    let (_dir, state, owner, _grant, _cert, grantee_key, grantee) =
        crate::grants::tests::fixture().await;
    let before = enrollment(&state).unwrap();
    for principal in [None, Some(&owner)] {
        assert_eq!(
            crate::grants::tests::request(
                &state,
                principal,
                &session_key(),
                "POST",
                "/api/transport/bind",
                remote_bind_input()
            )
            .await,
            (409, json!({"code":"already_bound"}))
        );
    }
    assert_eq!(
        crate::grants::tests::request(
            &state,
            Some(&grantee),
            &grantee_key,
            "POST",
            "/api/transport/bind",
            remote_bind_input()
        )
        .await,
        (403, json!({"code":"owner_required"}))
    );
    assert_eq!(enrollment(&state).unwrap(), before);
}
#[tokio::test]
async fn remote_bind_refuses_plaintext_peer_and_non_tunnel_requests() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let plain=crate::tunnel::forward_local(&state,json!({"method":"POST","path":"/api/transport/bind","body":remote_bind_input().to_string()}),None).await;
    assert_eq!(plain["statusCode"], 403);
    assert_eq!(
        serde_json::from_str::<Value>(plain["body"].as_str().unwrap()).unwrap()["code"],
        "device_strict_mode"
    );
    let (payload, _key, _aad) = encrypted(
        &state,
        "POST",
        "/api/transport/bind",
        remote_bind_input(),
        None,
    );
    let peer = crate::auth::PeerCaller {
        device_id: "peer".into(),
        device_name: "Peer".into(),
        user_id: "owner".into(),
    };
    let denied = crate::tunnel::forward_local(&state, payload, Some(peer)).await;
    assert_eq!(denied["statusCode"], 403);
    assert_eq!(
        serde_json::from_str::<Value>(denied["body"].as_str().unwrap()).unwrap()["code"],
        "owner_required"
    );
    for encrypted_extension in [false, true] {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/transport/bind")
            .header("content-type", "application/json");
        if encrypted_extension {
            request = request.extension(EncryptedOwnerRequest);
        }
        let response = crate::http::router(state.clone())
            .oneshot(
                request
                    .body(axum::body::Body::from(remote_bind_input().to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    assert!(!bound(&state));
}
#[tokio::test]
async fn remote_bind_concurrent_first_requests_never_replace_the_winning_key() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    remember_device(&state, "test-device").unwrap();
    let first = remote_bind_input();
    let mut other = first.clone();
    other.as_object_mut().unwrap().remove("webOrigin");
    other["accountPublicKey"] = json!(account(&SigningKey::random(&mut OsRng)).account_public_key);
    let key = session_key();
    let (a, b) = tokio::join!(
        crate::grants::tests::request(
            &state,
            None,
            &key,
            "POST",
            "/api/transport/bind",
            first.clone()
        ),
        crate::grants::tests::request(
            &state,
            None,
            &key,
            "POST",
            "/api/transport/bind",
            other.clone()
        )
    );
    assert!(
        (a.0 == 200 && b == (409, json!({"code":"already_bound"})))
            || (b.0 == 200 && a == (409, json!({"code":"already_bound"})))
    );
    let winner = if a.0 == 200 { first } else { other };
    assert_eq!(
        pinned_account(&state).unwrap().unwrap().account_public_key,
        winner["accountPublicKey"]
    );
}
#[tokio::test]
async fn remote_bind_capability_is_present_in_device_and_peer_descriptors() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    state.set_peer_access(true).unwrap();
    let peer = crate::auth::PeerCaller {
        device_id: "peer".into(),
        device_name: "Peer".into(),
        user_id: "owner".into(),
    };
    for (path, caller) in [
        ("/api/transport/key", None),
        ("/api/peer/transport/key", Some(peer)),
    ] {
        let response =
            crate::tunnel::forward_local(&state, json!({"method":"GET","path":path}), caller).await;
        assert_eq!(response["statusCode"], 200);
        let descriptor: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
        assert_eq!(descriptor["trust"]["capabilities"], json!(["remote-bind"]));
    }
}
#[tokio::test]
async fn remote_bind_does_not_log_or_echo_enrollment_secrets() {
    use std::io::Write;
    use tracing::instrument::WithSubscriber;
    #[derive(Clone)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || Capture(captured.clone()))
        .finish();
    async {
        tracing::info!("remote-bind capture probe");
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        remember_device(&state, "test-device").unwrap();
        let mut bad = remote_bind_input();
        bad["enrollSecret"] = json!("invalid-sensitive-enrollment-marker");
        let (status, body) = crate::grants::tests::request(
            &state,
            None,
            &session_key(),
            "POST",
            "/api/transport/bind",
            bad,
        )
        .await;
        assert_eq!((status, &body), (400, &json!({"code":"invalid_bind"})));
        assert!(!body
            .to_string()
            .contains("invalid-sensitive-enrollment-marker"));
        assert!(!bound(&state));
        let (status, body) = crate::grants::tests::request(
            &state,
            None,
            &session_key(),
            "POST",
            "/api/transport/bind",
            remote_bind_input(),
        )
        .await;
        assert_eq!(status, 200);
        assert!(!body.to_string().contains(&B64.encode([59; 32])));
    }
    .with_subscriber(subscriber)
    .await;
    let logs = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("remote-bind capture probe"));
    assert!(!logs.contains("invalid-sensitive-enrollment-marker"));
    assert!(!logs.contains(&B64.encode([59; 32])));
}

pub(crate) fn vectors() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/account-key-v2.json")).unwrap()
}
fn signing(key: &Value) -> SigningKey {
    SigningKey::from_pkcs8_der(&B64.decode(key.as_str().unwrap()).unwrap()).unwrap()
}
pub(crate) fn account_key() -> SigningKey {
    signing(&vectors()["account"]["accountPrivateKeyPkcs8"])
}
pub(crate) fn session_key() -> SigningKey {
    signing(&vectors()["browserCertificate"]["browserPrivateKeyPkcs8"])
}
pub(crate) fn account(key: &SigningKey) -> PinnedAccount {
    let public = B64.encode(key.verifying_key().to_public_key_der().unwrap().as_bytes());
    PinnedAccount {
        account_key_id: account_key_id(&public).unwrap(),
        account_public_key: public,
    }
}
pub(crate) fn browser_certificate(
    account_key: &SigningKey,
    browser_key: &SigningKey,
) -> BrowserCertificate {
    let pin = account(account_key);
    let public_key = account(browser_key).account_public_key;
    let mut c = BrowserCertificate {
        version: 1,
        account_key_id: pin.account_key_id,
        browser_id: account_key_id(&public_key).unwrap(),
        public_key,
        created_at: now(),
        signature: String::new(),
    };
    let signature: Signature = account_key.sign(browser_signing_input(&c).as_bytes());
    c.signature = B64.encode(signature.to_bytes());
    c
}
pub(crate) fn browser_revocation(principal: &BrowserPrincipal) -> Revocation {
    let mut r = Revocation {
        version: 1,
        account_key_id: principal.account_key_id.clone(),
        kind: "browser".into(),
        subject_id: principal.browser_id.clone(),
        revoked_at: now(),
        signature: String::new(),
    };
    let sig: Signature = account_key().sign(revocation_signing_input(&r).as_bytes());
    r.signature = B64.encode(sig.to_bytes());
    r
}
pub(crate) fn live_owner(state: &Supervisor) -> BrowserPrincipal {
    let v = vectors();
    bind_account(
        &state.config.database_url,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        v["enrollment"]["enrollSecret"].as_str().unwrap(),
        None,
    )
    .unwrap();
    remember_device(state, "test-device").unwrap();
    let c: BrowserCertificate =
        serde_json::from_value(v["browserCertificate"]["certificate"].clone()).unwrap();
    register(&state.db, &c).unwrap();
    BrowserPrincipal {
        browser_id: c.browser_id,
        account_key_id: c.account_key_id,
        grantee_user_id: None,
    }
}
pub(crate) fn bound_peer(state: &Supervisor, id: &str) -> DeviceCertificate {
    bound_peer_with_account(state, id, &account_key())
}
pub(crate) fn bound_peer_with_account(
    state: &Supervisor,
    id: &str,
    key: &SigningKey,
) -> DeviceCertificate {
    let pin = account(key);
    bind_account(
        &state.config.database_url,
        &pin.account_public_key,
        vectors()["enrollment"]["enrollSecret"].as_str().unwrap(),
        None,
    )
    .unwrap();
    remember_device(state, id).unwrap();
    let mut c = DeviceCertificate {
        version: 1,
        account_key_id: pin.account_key_id,
        device_id: id.into(),
        identity_key: crate::secure_transport::transport(state)
            .unwrap()
            .identity_key(),
        created_at: now(),
        signature: String::new(),
    };
    let sig: Signature = key.sign(device_signing_input(&c).as_bytes());
    c.signature = B64.encode(sig.to_bytes());
    install_certificate(state, &c).unwrap();
    c
}
pub(crate) fn device_revocation(id: &str) -> Revocation {
    let mut r: Revocation =
        serde_json::from_value(vectors()["revocations"]["device"].clone()).unwrap();
    r.subject_id = id.into();
    let signature: Signature = account_key().sign(revocation_signing_input(&r).as_bytes());
    r.signature = B64.encode(signature.to_bytes());
    r
}
/// A browser-like HPKE request, with a response exporter, tests actual tunnel dispatch.
pub(crate) fn encrypted(
    state: &Supervisor,
    method: &str,
    path: &str,
    body: Value,
    owner: Option<&BrowserPrincipal>,
) -> (Value, [u8; 32], String) {
    encrypted_with_key(state, method, path, body, owner, &session_key())
}
pub(crate) fn encrypted_with_key(
    state: &Supervisor,
    method: &str,
    path: &str,
    body: Value,
    principal: Option<&BrowserPrincipal>,
    signing_key: &SigningKey,
) -> (Value, [u8; 32], String) {
    let transport = crate::secure_transport::transport(state).unwrap();
    let descriptor = transport.descriptor("").unwrap();
    encrypted_with_descriptor(method, path, body, principal, signing_key, &descriptor)
}
pub(crate) fn encrypted_with_descriptor(
    method: &str,
    path: &str,
    body: Value,
    principal: Option<&BrowserPrincipal>,
    signing_key: &SigningKey,
    d: &Value,
) -> (Value, [u8; 32], String) {
    use hpke::{
        aead::AesGcm256, kdf::HkdfSha256, kem::DhP256HkdfSha256 as Kem, Deserializable,
        Kem as KemTrait, Serializable,
    };
    let public = <Kem as KemTrait>::PublicKey::from_bytes(
        &B64.decode(d["publicKey"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    use rand09::SeedableRng;
    let mut rng = rand09::rngs::StdRng::from_os_rng();
    let (enc, mut sender) = hpke::setup_sender::<AesGcm256, HkdfSha256, Kem, _>(
        &hpke::OpModeS::Base,
        &public,
        b"remote-codex/relay/v1",
        &mut rng,
    )
    .unwrap();
    let id = format!("{}.{}", uuid::Uuid::new_v4(), now());
    let key = d["keyId"].as_str().unwrap();
    let aad = format!("rcd-http-v1\n{key}\n{id}\n{method}\n{path}\n");
    let meta = serde_json::to_vec(&json!({"headers":{"content-type":"application/json"}})).unwrap();
    let mut clear = (meta.len() as u32).to_be_bytes().to_vec();
    clear.extend(meta);
    if !body.is_null() {
        clear.extend(serde_json::to_vec(&body).unwrap());
    }
    let sealed = sender.seal(&clear, aad.as_bytes()).unwrap();
    let mut response = [0; 32];
    sender
        .export(b"remote-codex/http-response/v1", &mut response)
        .unwrap();
    let mut request = json!({"method":method,"path":path,"headers":{"x-rcd-key":key,"x-rcd-request":id,"x-rcd-enc":B64.encode(enc.to_bytes()),"x-rcd-sealed":B64.encode(&sealed)}});
    if let Some(owner) = principal {
        let sig: Signature =
            signing_key.sign(format!("pockymoe-request-v1\n{aad}\n{}", hash(&sealed)).as_bytes());
        request["headers"]["x-rcd-session"] = json!(owner.browser_id);
        request["headers"]["x-rcd-signature"] = json!(B64.encode(sig.to_bytes()));
    }
    (request, response, aad)
}
pub(crate) fn decoded(response: Value, key: &[u8; 32], aad: &str) -> (u64, Value) {
    let (meta, body) = decoded_parts(response, key, aad);
    (
        meta["status"].as_u64().unwrap(),
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    )
}
pub(crate) fn decoded_parts(response: Value, key: &[u8; 32], aad: &str) -> (Value, Vec<u8>) {
    use aes_gcm::{
        aead::{Aead, Payload},
        Aes256Gcm, KeyInit, Nonce,
    };
    assert_eq!(response["headers"]["x-rcd-encrypted"], "1", "{response}");
    let clear = Aes256Gcm::new_from_slice(key)
        .unwrap()
        .decrypt(
            Nonce::from_slice(&[0; 12]),
            Payload {
                msg: &base64::engine::general_purpose::STANDARD
                    .decode(response["body"].as_str().unwrap())
                    .unwrap(),
                aad: format!("{aad}\nresponse").as_bytes(),
            },
        )
        .unwrap();
    let n = u32::from_be_bytes(clear[..4].try_into().unwrap()) as usize;
    let meta: Value = serde_json::from_slice(&clear[4..4 + n]).unwrap();
    (meta, clear[4 + n..].to_vec())
}

#[test]
fn v2_certificate_request_peer_and_enrollment_vectors_including_every_negative() {
    let v = vectors();
    let pin: PinnedAccount = serde_json::from_value(v["account"].clone()).unwrap();
    assert_eq!(
        account_key_id(&pin.account_public_key).unwrap(),
        pin.account_key_id
    );
    let browser: BrowserCertificate =
        serde_json::from_value(v["browserCertificate"]["certificate"].clone()).unwrap();
    verify_browser_certificate(&pin, &browser).unwrap();
    assert_eq!(
        browser_signing_input(&browser),
        v["browserCertificate"]["signingInput"]
    );
    let device: DeviceCertificate =
        serde_json::from_value(v["deviceCertificate"]["certificate"].clone()).unwrap();
    verify_device(&pin, &device, &device.device_id, &device.identity_key).unwrap();
    assert_eq!(
        device_signing_input(&device),
        v["deviceCertificate"]["signingInput"]
    );
    for kind in ["browser", "device"] {
        let r: Revocation = serde_json::from_value(v["revocations"][kind].clone()).unwrap();
        verify_revocation(&pin, &r).unwrap();
    }
    let req = &v["requestSignature"];
    assert_eq!(req["sessionHeader"], browser.browser_id);
    verify_request(
        &browser.public_key,
        req["aad"].as_str().unwrap(),
        &B64.decode(req["sealed"].as_str().unwrap()).unwrap(),
        req["signature"].as_str().unwrap(),
    )
    .unwrap();
    let sealed = B64.decode(req["sealed"].as_str().unwrap()).unwrap();
    assert!(verify_request(
        &browser.public_key,
        "wrong-aad",
        &sealed,
        req["signature"].as_str().unwrap()
    )
    .is_err());
    assert!(verify_request(
        &browser.public_key,
        req["aad"].as_str().unwrap(),
        b"changed",
        req["signature"].as_str().unwrap()
    )
    .is_err());
    assert!(verify_request(
        &browser.public_key,
        req["aad"].as_str().unwrap(),
        &sealed,
        &B64.encode([0; 64])
    )
    .is_err());
    let raw =
        Signature::from_slice(&B64.decode(req["signature"].as_str().unwrap()).unwrap()).unwrap();
    assert!(verify_request(
        &browser.public_key,
        req["aad"].as_str().unwrap(),
        &sealed,
        &B64.encode(raw.to_der().as_bytes())
    )
    .is_err());
    let peer = &v["peerSignature"];
    let input = peer_signing_input(
        peer["callerDeviceId"].as_str().unwrap(),
        peer["aad"].as_str().unwrap(),
        &B64.decode(peer["sealed"].as_str().unwrap()).unwrap(),
    );
    assert_eq!(input, peer["signingInput"]);
    VerifyingKey::from_sec1_bytes(&B64.decode(&device.identity_key).unwrap())
        .unwrap()
        .verify(
            input.as_bytes(),
            &Signature::from_slice(&B64.decode(peer["signature"].as_str().unwrap()).unwrap())
                .unwrap(),
        )
        .unwrap();
    let mac = enrollment_mac(
        &B64.decode(v["enrollment"]["enrollSecret"].as_str().unwrap())
            .unwrap(),
        &device.device_id,
        &device.identity_key,
        &pin.account_key_id,
    )
    .unwrap();
    assert_eq!(mac, v["enrollment"]["mac"]);
    for (name, negative) in v["negative"].as_object().unwrap() {
        match name.as_str() {
            n if n.starts_with("browserCert") => assert!(
                verify_browser_certificate(
                    &pin,
                    &serde_json::from_value(negative.clone()).unwrap()
                )
                .is_err(),
                "{name}"
            ),
            n if n.starts_with("deviceCert") => assert!(
                verify_device(
                    &pin,
                    &serde_json::from_value(negative.clone()).unwrap(),
                    &device.device_id,
                    &device.identity_key
                )
                .is_err(),
                "{name}"
            ),
            "revocationWrongKind" => assert!(verify_revocation(
                &pin,
                &serde_json::from_value(negative.clone()).unwrap()
            )
            .is_err()),
            "enrollmentMacWrongIdentity" => assert_ne!(&json!(mac), negative),
            "vaultBodyWrongRevisionAad" => {
                use aes_gcm::{
                    aead::{Aead, Payload},
                    Aes256Gcm, KeyInit, Nonce,
                };
                let vault = &v["vault"];
                let aes =
                    Aes256Gcm::new_from_slice(&B64.decode(vault["vdk"].as_str().unwrap()).unwrap())
                        .unwrap();
                let iv = B64
                    .decode(vault["vault"]["body"]["iv"].as_str().unwrap())
                    .unwrap();
                let ciphertext = B64
                    .decode(vault["vault"]["body"]["ciphertext"].as_str().unwrap())
                    .unwrap();
                assert!(aes
                    .decrypt(
                        Nonce::from_slice(&iv),
                        Payload {
                            msg: &ciphertext,
                            aad: vault["bodyAad"].as_str().unwrap().as_bytes()
                        }
                    )
                    .is_ok());
                assert!(aes
                    .decrypt(
                        Nonce::from_slice(&iv),
                        Payload {
                            msg: &ciphertext,
                            aad: negative.as_str().unwrap().as_bytes()
                        }
                    )
                    .is_err());
            }
            _ => panic!("Uncovered negative vector {name}"),
        }
    }
}
#[test]
fn binding_offline_status_same_key_rebind_different_key_reset_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fresh.sqlite");
    let v = vectors();
    let public = v["account"]["accountPublicKey"].as_str().unwrap();
    let secret = v["enrollment"]["enrollSecret"].as_str().unwrap();
    assert_eq!(binding_status(&db).unwrap()["bound"], false);
    bind_account(&db, public, secret, Some("https://web.test")).unwrap();
    let status = binding_status(&db).unwrap();
    assert_eq!(status["bound"], true);
    assert_eq!(status["strict"], true);
    assert_eq!(status["accountKeyId"], v["account"]["accountKeyId"]);
    bind_account(&db, public, &B64.encode([9; 32]), None).unwrap();
    assert!(bind_account(
        &db,
        v["otherAccount"]["accountPublicKey"].as_str().unwrap(),
        secret,
        None
    )
    .is_err());
    assert!(bind_account(&db, public, secret, Some("https://web.test/path")).is_err());
    assert!(bind_account(&db, "invalid", secret, None).is_err());
    assert!(bind_account(&db, public, "invalid", None).is_err());
    let c = Connection::open(&db).unwrap();
    assert_eq!(
        c.query_row("SELECT enroll_secret FROM device_owner_trust", [], |r| r
            .get::<_, Vec<u8>>(0))
            .unwrap(),
        vec![9; 32]
    );
    assert_eq!(
        c.query_row("SELECT web_origin FROM device_owner_trust", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "https://web.test"
    );
    drop(c);
    reset_account_trust(&db).unwrap();
    assert_eq!(
        binding_status(&db).unwrap(),
        json!({"bound":false,"strict":false,"accountKeyId":null})
    );
    bind_account(
        &db,
        v["otherAccount"]["accountPublicKey"].as_str().unwrap(),
        secret,
        None,
    )
    .unwrap();
}
#[tokio::test]
async fn enrollment_certificate_install_and_descriptor_persist() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let (status, e) = crate::grants::tests::request(
        &state,
        None,
        &session_key(),
        "GET",
        "/api/transport/enrollment",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        e["mac"],
        enrollment_mac(
            &B64.decode(vectors()["enrollment"]["enrollSecret"].as_str().unwrap())
                .unwrap(),
            "test-device",
            e["identityKey"].as_str().unwrap(),
            &owner.account_key_id
        )
        .unwrap()
    );
    let mut cert = DeviceCertificate {
        version: 1,
        device_id: "test-device".into(),
        identity_key: e["identityKey"].as_str().unwrap().into(),
        account_key_id: owner.account_key_id.clone(),
        created_at: now(),
        signature: String::new(),
    };
    let sig: Signature = account_key().sign(device_signing_input(&cert).as_bytes());
    cert.signature = B64.encode(sig.to_bytes());
    for field in ["deviceId", "identityKey", "accountKeyId", "signature"] {
        let mut bad = serde_json::to_value(&cert).unwrap();
        bad[field] = json!("wrong");
        let (status, body) = crate::grants::tests::request(
            &state,
            Some(&owner),
            &session_key(),
            "PUT",
            "/api/transport/device-certificate",
            bad,
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("invalid_certificate")),
            "{field}"
        );
    }
    let (status, body) = crate::grants::tests::request(
        &state,
        Some(&owner),
        &session_key(),
        "PUT",
        "/api/transport/device-certificate",
        serde_json::to_value(&cert).unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["certificate"], serde_json::to_value(&cert).unwrap());
    assert_eq!(
        crate::grants::tests::request(
            &state,
            None,
            &session_key(),
            "GET",
            "/api/transport/enrollment",
            Value::Null
        )
        .await,
        (404, json!({"code":"enrollment_missing"}))
    );
    let response = crate::tunnel::forward_local(
        &state,
        json!({"method":"GET","path":"/api/transport/key"}),
        None,
    )
    .await;
    let d: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
    assert_eq!(d["certificate"], body["certificate"]);
    assert_eq!(d["trust"], hint(&state).unwrap());
    assert_eq!(
        device_certificate(&state).unwrap().unwrap().signature,
        cert.signature
    );
}
#[tokio::test]
async fn strict_plain_unsigned_signed_revoked_and_allowed_exceptions() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let p = live_owner(&state);
    for (method, path) in [
        ("GET", "/api/version"),
        ("POST", "/api/transport/browser"),
        ("GET", "/api/transport/enrollment"),
        ("PUT", "/api/transport/revocations"),
        ("POST", "/healthz"),
        ("GET", "/api/peer/transport/key"),
    ] {
        let response =
            crate::tunnel::forward_local(&state, json!({"method":method,"path":path}), None).await;
        assert_eq!(response["statusCode"], 403, "{path}");
        assert_eq!(
            serde_json::from_str::<Value>(response["body"].as_str().unwrap()).unwrap()["code"],
            "device_strict_mode"
        );
    }
    for path in [
        "/api/version",
        "/api/transport/trust",
        "/api/transport/session",
        "/api/transport/stream/invalid",
        "/api/port-mappings",
    ] {
        let (status, body) =
            crate::grants::tests::request(&state, None, &session_key(), "GET", path, Value::Null)
                .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (401, Some("browser_registration_required")),
            "{path}"
        );
    }
    assert_eq!(
        crate::grants::tests::request(
            &state,
            Some(&p),
            &session_key(),
            "GET",
            "/api/version",
            Value::Null
        )
        .await
        .0,
        200
    );
    let (mut r, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, Some(&p));
    r["headers"]["x-rcd-signature"] = json!(B64.encode([0; 64]));
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        401
    );
    for path in [
        "/healthz",
        "/api/transport/key",
        "/api/publications/missing",
    ] {
        let r =
            crate::tunnel::forward_local(&state, json!({"method":"GET","path":path}), None).await;
        assert_ne!(r["statusCode"], 401);
        assert_ne!(r["statusCode"], 403, "{path}");
    }
    let (status, body) = crate::grants::tests::request(
        &state,
        Some(&p),
        &session_key(),
        "GET",
        "/api/transport/trust",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["browsers"][0]["current"], true);
    assert_eq!(body["bound"], true);
    assert_eq!(
        crate::grants::tests::request(
            &state,
            None,
            &session_key(),
            "POST",
            "/api/transport/browser",
            vectors()["browserCertificate"]["certificate"].clone()
        )
        .await
        .0,
        200
    );
    assert_eq!(
        crate::grants::tests::request(
            &state,
            None,
            &session_key(),
            "PUT",
            "/api/transport/revocations",
            json!({"revocations":[]})
        )
        .await
        .0,
        200
    );
    for (method, path) in [
        ("POST", "/api/transport/enrollment"),
        ("GET", "/api/transport/browser"),
        ("GET", "/api/transport/revocations"),
    ] {
        assert_eq!(
            crate::grants::tests::request(&state, None, &session_key(), method, path, Value::Null)
                .await
                .0,
            401
        );
    }
    apply_revocations(&state, &[browser_revocation(&p)]).unwrap();
    assert!(!p.valid(&state));
    let result = crate::grants::tests::request(
        &state,
        Some(&p),
        &session_key(),
        "GET",
        "/api/version",
        Value::Null,
    )
    .await;
    assert_eq!(result, (401, json!({"code":"browser_revoked"})));
    assert_eq!(
        crate::grants::tests::request(
            &state,
            None,
            &session_key(),
            "POST",
            "/api/transport/browser",
            vectors()["browserCertificate"]["certificate"].clone()
        )
        .await,
        (401, json!({"code":"browser_revoked"}))
    );
}
#[tokio::test]
async fn revocations_verified_entry_by_entry_permanent_and_close_live_channels() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let p = live_owner(&state);
    let (status, c) = crate::grants::tests::request(
        &state,
        Some(&p),
        &session_key(),
        "POST",
        "/api/transport/session",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let t = crate::secure_transport::transport(&state).unwrap();
    let channel = t.session(c["channelId"].as_str().unwrap()).unwrap();
    assert!(channel.alive());
    assert!(channel.principal.as_ref().unwrap().valid(&state));
    let r = browser_revocation(&p);
    let mut bad = r.clone();
    bad.kind = "device".into();
    apply_revocations(&state, &[bad, r.clone()]).unwrap();
    assert!(!p.valid(&state));
    let (status, _) = crate::grants::tests::request(
        &state,
        None,
        &session_key(),
        "PUT",
        "/api/transport/revocations",
        json!({"revocations":[r.clone()]}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(!channel.alive());
    assert!(t.session(c["channelId"].as_str().unwrap()).is_none());
    assert!(channel.seal(&json!({"type":"private.event"})).is_err());
    apply_revocations(&state, &[r]).unwrap();
    assert_eq!(trust(&state, "").unwrap()["revocations"], 1);
    assert_eq!(trust(&state, "").unwrap()["browsers"], json!([]));
    let persisted =
        Connection::open_with_flags(&state.config.database_url, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        persisted
            .query_row("SELECT count(*) FROM account_revocations", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        rejection_code(
            &register(
                &state.db,
                &serde_json::from_value(vectors()["browserCertificate"]["certificate"].clone())
                    .unwrap()
            )
            .unwrap_err()
        )
        .1,
        "browser_revoked"
    );
    reset_account_trust(&state.config.database_url).unwrap();
    assert_eq!(trust(&state, "").unwrap()["revocations"], 1);
    let v = vectors();
    bind_account(
        &state.config.database_url,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        v["enrollment"]["enrollSecret"].as_str().unwrap(),
        None,
    )
    .unwrap();
    let cert = serde_json::from_value(v["browserCertificate"]["certificate"].clone()).unwrap();
    assert_eq!(
        rejection_code(&register(&state.db, &cert).unwrap_err()).1,
        "browser_revoked"
    );
}
#[tokio::test]
async fn unknown_account_and_bad_browser_certificates_fail_registration() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    live_owner(&state);
    let account_key = SigningKey::random(&mut OsRng);
    let cert = browser_certificate(&account_key, &SigningKey::random(&mut OsRng));
    assert_eq!(
        crate::grants::tests::request(
            &state,
            None,
            &session_key(),
            "POST",
            "/api/transport/browser",
            serde_json::to_value(cert).unwrap()
        )
        .await,
        (403, json!({"code":"account_mismatch"}))
    );
    for name in [
        "browserCertSignedByOtherAccount",
        "browserCertWrongBrowserId",
        "browserCertTamperedCreatedAt",
    ] {
        assert_eq!(
            crate::grants::tests::request(
                &state,
                None,
                &session_key(),
                "POST",
                "/api/transport/browser",
                vectors()["negative"][name].clone()
            )
            .await,
            (400, json!({"code":"invalid_certificate"}))
        );
    }
}
#[tokio::test]
async fn unbound_ignores_session_headers_and_retains_plaintext_behavior() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let response=crate::tunnel::forward_local(&state,json!({"method":"GET","path":"/api/version","headers":{"x-rcd-session":"unknown","x-rcd-signature":"invalid"}}),None).await;
    assert_eq!(response["statusCode"], 200);
    let (mut r, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, None);
    r["headers"]["x-rcd-session"] = json!("unknown");
    r["headers"]["x-rcd-signature"] = json!("invalid");
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        200
    );
    let p = live_owner(&state);
    let mut unknown = p;
    unknown.browser_id = "unknown".into();
    assert_eq!(
        crate::grants::tests::request(
            &state,
            Some(&unknown),
            &session_key(),
            "GET",
            "/api/version",
            Value::Null
        )
        .await,
        (401, json!({"code":"browser_registration_required"}))
    );
}
#[tokio::test]
async fn trust_endpoints_are_encrypted_only_even_on_authenticated_local_listener() {
    use tower::ServiceExt;
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    live_owner(&state);
    let router = crate::http::router(state);
    for (method, path) in [
        ("POST", "/api/transport/browser"),
        ("GET", "/api/transport/enrollment"),
        ("PUT", "/api/transport/revocations"),
        ("PUT", "/api/transport/device-certificate"),
        ("GET", "/api/transport/trust"),
    ] {
        let r = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn file_save_actor_is_browser_id_and_ignores_relay_actor() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let ws = state
        .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
            git_url: None,
            label: None,
        })
        .unwrap();
    std::fs::write(state.config.workspace_root.join("owner.txt"), "before").unwrap();
    let doc = state.file_document(&ws.id, "owner.txt").unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let save = pockymoe_runtime::file_documents::SaveDocument {
        path: doc.path,
        workspace_revision: doc.workspace_revision,
        file_identity: doc.file_identity,
        expected_hash: doc.content_hash.unwrap(),
        content: "after".into(),
        draft_revision: 1,
        operation_id: operation.clone(),
        operation_created_at: now() as u64,
    };
    let (mut r, key, aad) = encrypted(
        &state,
        "POST",
        &format!("/api/workspaces/{}/files/save", ws.id),
        serde_json::to_value(save).unwrap(),
        Some(&owner),
    );
    r["headers"]["x-rcd-file-actor"] = json!("forged");
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        200
    );
    assert!(state
        .file_operation(&owner.actor(), &ws.id, &operation)
        .is_ok());
    assert!(state
        .file_operation("relay:forged", &ws.id, &operation)
        .is_err());
}

#[test]
fn bind_prepares_version_13_before_setup_restarts_its_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let db = Database::open(&path).unwrap();
    db.with(|c| { c.execute_batch("DROP TABLE device_owner_trust; DROP TABLE browser_registrations; DROP TABLE account_revocations; DROP TABLE device_certificates; DROP TABLE owner_port_mappings; DROP TABLE device_grants; DROP TABLE peer_certificates; DROP TABLE preview_leases; DELETE FROM __remote_codex_runtime_migrations WHERE version>=14;")?; Ok(()) }).unwrap();
    assert_eq!(binding_status(&path).unwrap()["bound"], false);
    let v = vectors();
    bind_account(
        &path,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        v["enrollment"]["enrollSecret"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(binding_status(&path).unwrap()["bound"], true);
    db.set_kv("still-owned", "yes").unwrap();
    assert_eq!(
        db.with(|c| Ok(c.query_row(
            "SELECT max(version) FROM __remote_codex_runtime_migrations",
            [],
            |r| r.get::<_, u32>(0)
        )?))
        .unwrap(),
        16
    );
}

#[test]
fn web_origin_keeps_https_and_loopback_validation() {
    for origin in [
        "https://web.test",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
        "http://[::1]:3000",
    ] {
        assert!(validate_web_origin(origin).is_ok(), "{origin}");
    }
    for origin in [
        "http://web.test",
        "https://web.test/path",
        "https://web.test?query",
        "https://web.test#fragment",
        "https://user:secret@web.test",
        "invalid",
    ] {
        assert!(validate_web_origin(origin).is_err());
    }
}

#[tokio::test]
async fn mixed_revocation_json_ignores_unknown_keys_and_malformed_entries() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let valid = browser_revocation(&owner);
    let mut other = valid.clone();
    other.account_key_id = account(&SigningKey::random(&mut OsRng)).account_key_id;
    let mut invalid = valid.clone();
    invalid.signature = B64.encode([0; 64]);
    let (status, _) = crate::grants::tests::request(
        &state,
        None,
        &session_key(),
        "PUT",
        "/api/transport/revocations",
        json!({"revocations":[other, {"malformed":true}, invalid, valid]}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(!owner.valid(&state));
    assert_eq!(trust(&state, "").unwrap()["revocations"], 1);
}
#[test]
fn binding_deletes_only_keyless_publications() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let policy = json!({"threadId":"unused","turnIds":[],"afterOrdinal":0,"theme":"dark","createdAt":"unused"});
    state
        .db
        .set_kv("publication:legacy", &policy.to_string())
        .unwrap();
    let mut encrypted = policy.clone();
    encrypted["encryptionKey"] = json!(B64.encode([0; 32]));
    state
        .db
        .set_kv("publication:encrypted", &encrypted.to_string())
        .unwrap();
    live_owner(&state);
    assert!(state.db.get_kv("publication:legacy").unwrap().is_none());
    assert!(state.db.get_kv("publication:encrypted").unwrap().is_some());
}
#[cfg(unix)]
#[test]
fn binding_restricts_existing_database_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    std::fs::set_permissions(
        &state.config.database_url,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    for suffix in ["-wal", "-shm"] {
        let file =
            std::path::PathBuf::from(format!("{}{suffix}", state.config.database_url.display()));
        assert!(file.exists());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    live_owner(&state);
    for suffix in ["-wal", "-shm"] {
        let file =
            std::path::PathBuf::from(format!("{}{suffix}", state.config.database_url.display()));
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::metadata(&state.config.database_url)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[tokio::test]
async fn second_review_mismatch_status_write_failure_keeps_tunnel_retrying() {
    use futures_util::SinkExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url(&format!(
        "http://{}",
        listener.local_addr().unwrap()
    ));
    live_owner(&state);
    state.db.with(|c| {
        c.execute_batch("CREATE TRIGGER fail_retry_status BEFORE UPDATE ON kv WHEN NEW.key='relayTrustStatus' AND json_extract(NEW.value,'$.retryAt') IS NOT NULL BEGIN SELECT RAISE(FAIL,'synthetic retry status write failure'); END;")?;
        Ok(())
    }).unwrap();
    let tunnel = tokio::spawn(crate::tunnel::run_relay_tunnel(state.clone()));
    let mut times = Vec::new();
    for _ in 0..3 {
        let accepted =
            tokio::time::timeout(std::time::Duration::from_secs(4), listener.accept()).await;
        if accepted.is_err() {
            tunnel.abort();
            panic!("tunnel stopped retrying after status persistence failed");
        }
        let (socket, _) = accepted.unwrap().unwrap();
        times.push(tokio::time::Instant::now());
        let mut relay = tokio_tungstenite::accept_async(socket).await.unwrap();
        relay
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"type":"relay.connected","deviceId":"different-device"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    }
    tunnel.abort();
    assert!(times[1].duration_since(times[0]) >= std::time::Duration::from_secs(1));
    assert!(times[2].duration_since(times[1]) >= std::time::Duration::from_secs(2));
}

#[tokio::test]
async fn relay_device_id_mismatch_backs_off_and_reports_status() {
    use futures_util::SinkExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url(&format!(
        "http://{}",
        listener.local_addr().unwrap()
    ));
    live_owner(&state);
    let tunnel = tokio::spawn(crate::tunnel::run_relay_tunnel(state.clone()));
    let mut times = Vec::new();
    for _ in 0..3 {
        let (socket, _) =
            tokio::time::timeout(std::time::Duration::from_secs(8), listener.accept())
                .await
                .unwrap()
                .unwrap();
        times.push(tokio::time::Instant::now());
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"type":"relay.connected","deviceId":"different-device"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    tunnel.abort();
    let response = crate::http::router(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri("/healthz")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(times[1].duration_since(times[0]) >= std::time::Duration::from_secs(1));
    assert!(times[2].duration_since(times[1]) >= std::time::Duration::from_secs(2));
    assert_eq!(status["relayStatus"]["code"], "device_id_mismatch");
    assert!(status["relayStatus"]["retryAt"].as_i64().unwrap() > now());
    assert!(!state
        .relay_connected
        .load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(state.db.with(device_id).unwrap(), "test-device");
    remember_device(&state, "test-device").unwrap();
}

#[test]
fn reset_preserves_tombstones_scoped_to_the_account_key() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    apply_revocations(&state, &[browser_revocation(&owner)]).unwrap();
    reset_account_trust(&state.config.database_url).unwrap();
    let v = vectors();
    bind_account(
        &state.config.database_url,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        v["enrollment"]["enrollSecret"].as_str().unwrap(),
        None,
    )
    .unwrap();
    let c = browser_certificate(&account_key(), &session_key());
    assert_eq!(
        rejection_code(&register(&state.db, &c).unwrap_err()).1,
        "browser_revoked"
    );
    reset_account_trust(&state.config.database_url).unwrap();
    let other = SigningKey::random(&mut OsRng);
    let account = account(&other);
    bind_account(
        &state.config.database_url,
        &account.account_public_key,
        v["enrollment"]["enrollSecret"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert!(register(&state.db, &browser_certificate(&other, &session_key())).is_ok());
}

#[tokio::test]
async fn key_descriptors_accept_base64url_challenges_and_validate_supplied_lengths() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    for challenge in [
        "abcd_E-F".to_owned(),
        B64.encode([255; 32]),
        "a".repeat(128),
    ] {
        let response = crate::tunnel::forward_local(
            &state,
            json!({"method":"GET","path":format!("/api/transport/key?challenge={challenge}")}),
            None,
        )
        .await;
        assert_eq!(response["statusCode"], 200);
        let body: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["challenge"], challenge);
    }
    for challenge in [
        "".to_owned(),
        "short".to_owned(),
        "a".repeat(129),
        "invalid%2Fchallenge".to_owned(),
        "invalid%0Achallenge".to_owned(),
    ] {
        let response = crate::tunnel::forward_local(
            &state,
            json!({"method":"GET","path":format!("/api/transport/key?challenge={challenge}")}),
            None,
        )
        .await;
        assert_eq!(response["statusCode"], 400);
    }
}
#[tokio::test]
async fn bound_device_missing_certificate_can_bootstrap_enrollment_after_reset() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let prior = bound_peer(&state, "test-device");
    reset_account_trust(&state.config.database_url).unwrap();
    let v = vectors();
    let fresh_secret = [31; 32];
    bind_account(
        &state.config.database_url,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        &B64.encode(fresh_secret),
        None,
    )
    .unwrap();
    let response = crate::tunnel::forward_local(
        &state,
        json!({"method":"GET","path":"/api/transport/key?challenge=repair_nonce"}),
        None,
    )
    .await;
    assert_eq!(response["statusCode"], 200);
    let descriptor: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
    assert_eq!(descriptor["certificate"], Value::Null);
    assert_eq!(descriptor["identityKey"], prior.identity_key);
    let (r, key, aad) = encrypted_with_descriptor(
        "GET",
        "/api/transport/enrollment",
        Value::Null,
        None,
        &session_key(),
        &descriptor,
    );
    let (status, enrollment) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(
        enrollment["mac"],
        enrollment_mac(
            &fresh_secret,
            "test-device",
            &prior.identity_key,
            &prior.account_key_id
        )
        .unwrap()
    );
    let owner = live_owner(&state);
    assert_eq!(
        crate::grants::tests::request(
            &state,
            Some(&owner),
            &session_key(),
            "PUT",
            "/api/transport/device-certificate",
            serde_json::to_value(prior).unwrap()
        )
        .await
        .0,
        200
    );
}

#[test]
fn rebind_healthy_certificate_clears_enrollment_and_exposes_web_origin_hint() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let certificate = bound_peer(&state, "test-device");
    let v = vectors();
    bind_account(
        &state.config.database_url,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        &B64.encode([31; 32]),
        Some("https://web.test"),
    )
    .unwrap();
    assert_eq!(
        rejection_code(&enrollment(&state).unwrap_err()).1,
        "enrollment_missing"
    );
    assert_eq!(
        device_certificate(&state).unwrap().unwrap().signature,
        certificate.signature
    );
    assert!(state
        .db
        .with(|c| Ok(
            c.query_row("SELECT enroll_secret FROM device_owner_trust", [], |r| {
                r.get::<_, Option<Vec<u8>>>(0)
            })?
        ))
        .unwrap()
        .is_none());
    assert_eq!(
        binding_status(&state.config.database_url).unwrap()["webOrigin"],
        "https://web.test"
    );
}
#[test]
fn enrollment_cleans_up_stale_secret_for_a_certified_device() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    bound_peer(&state, "test-device");
    state
        .db
        .with(|c| {
            c.execute(
                "UPDATE device_owner_trust SET enroll_secret=?1",
                [vec![31u8; 32]],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        rejection_code(&enrollment(&state).unwrap_err()).1,
        "enrollment_missing"
    );
    assert!(state
        .db
        .with(|c| Ok(
            c.query_row("SELECT enroll_secret FROM device_owner_trust", [], |r| {
                r.get::<_, Option<Vec<u8>>>(0)
            })?
        ))
        .unwrap()
        .is_none());
}
#[tokio::test]
async fn rebind_after_lost_identity_rotates_transport_and_accepts_repaired_certificate() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let old = bound_peer(&state, "test-device");
    let owner = live_owner(&state);
    let (status, body) = crate::grants::tests::request(
        &state,
        Some(&owner),
        &session_key(),
        "POST",
        "/api/transport/session",
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    let old_transport = crate::secure_transport::transport(&state).unwrap();
    let old_channel = old_transport
        .session(body["channelId"].as_str().unwrap())
        .unwrap();
    std::fs::remove_file(
        state
            .config
            .database_url
            .with_extension("transport-identity"),
    )
    .unwrap();
    let secret = B64.encode([31; 32]);
    let v = vectors();
    bind_account(
        &state.config.database_url,
        v["account"]["accountPublicKey"].as_str().unwrap(),
        &secret,
        None,
    )
    .unwrap();
    let next = crate::secure_transport::transport(&state)
        .unwrap()
        .identity_key();
    assert_ne!(next, old.identity_key);
    assert!(!old_channel.alive());
    assert!(device_certificate(&state).unwrap().is_none());
    let enrollment = enrollment(&state).unwrap();
    assert_eq!(
        enrollment["mac"],
        enrollment_mac(&[31; 32], "test-device", &next, &owner.account_key_id).unwrap()
    );
    assert!(install_certificate(&state, &old).is_err());
    let mut repaired = old;
    repaired.identity_key = next;
    repaired.created_at = now();
    let signature: Signature = account_key().sign(device_signing_input(&repaired).as_bytes());
    repaired.signature = B64.encode(signature.to_bytes());
    install_certificate(&state, &repaired).unwrap();
    assert_eq!(
        device_certificate(&state).unwrap().unwrap().identity_key,
        repaired.identity_key
    );
    assert_eq!(
        rejection_code(&super::enrollment(&state).unwrap_err()).1,
        "enrollment_missing"
    );
}
