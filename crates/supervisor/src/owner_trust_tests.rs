use super::*;
use p256::{
    ecdsa::{signature::Signer, SigningKey},
    pkcs8::EncodePublicKey,
};

pub(crate) fn vectors() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/owner-trust-v1.json")).unwrap()
}
fn record() -> Credential {
    serde_json::from_value(vectors()["credentialRecord"].clone()).unwrap()
}
fn registration() -> Registration {
    serde_json::from_value(vectors()["session"]["registration"].clone()).unwrap()
}
fn vector_pair() -> Pairing {
    let v = vectors();
    Pairing {
        version: 1,
        pairing_id: v["deviceCertificate"]["certificate"]["pairingId"]
            .as_str()
            .unwrap()
            .into(),
        credential: record(),
        assertion: serde_json::from_value(
            v["deviceCertificate"]["certificate"]["assertion"].clone(),
        )
        .unwrap(),
        mac: v["pairing"]["mac"].as_str().unwrap().into(),
    }
}
pub(crate) fn session_key() -> SigningKey {
    SigningKey::from_slice(
        &B64.decode(vectors()["session"]["privateJwk"]["d"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
fn credential_key() -> SigningKey {
    SigningKey::from_slice(
        &B64.decode(vectors()["credentialPrivateJwk"]["d"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap()
}
fn assertion(c: &Credential, key: &SigningKey, challenge: &[u8]) -> Assertion {
    let client = serde_json::to_vec(
        &json!({"type":"webauthn.get","challenge":B64.encode(challenge),"origin":c.origin}),
    )
    .unwrap();
    let mut auth = Sha256::digest(c.rp_id.as_bytes()).to_vec();
    auth.extend_from_slice(&[5, 0, 0, 0, 0]);
    let mut input = auth.clone();
    input.extend_from_slice(&Sha256::digest(&client));
    let sig: Signature = key.sign(&input);
    Assertion {
        credential_id: c.credential_id.clone(),
        authenticator_data: B64.encode(auth),
        client_data_json: B64.encode(client),
        signature: B64.encode(sig.to_der().as_bytes()),
    }
}
fn resign_registration(r: &mut Registration) {
    r.assertion = assertion(&record(), &credential_key(), &session_challenge(r));
}
fn mac(p: &mut Pairing, device: &str, secret: &[u8]) {
    let mut m = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    m.update(pairing_mac_input(device, p).unwrap().as_bytes());
    p.mac = B64.encode(m.finalize().into_bytes());
}
fn fixture() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("db.sqlite")).unwrap();
    let v = vectors();
    let p = vector_pair();
    db.with(|c| {
        c.execute("UPDATE device_owner_trust SET relay_device_id=?1",[v["deviceCertificate"]["certificate"]["deviceId"].as_str().unwrap()])?;
        c.execute("INSERT INTO pairing_offers(pairing_id,secret,web_origin,rp_id,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6)", params![p.pairing_id,B64.decode(v["pairing"]["secret"].as_str().unwrap())?,p.credential.origin,p.credential.rp_id,0,600_000])?;
        Ok(())
    }).unwrap();
    (dir, db)
}
fn vector_identity() -> String {
    vectors()["deviceCertificate"]["certificate"]["identityKey"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn authoritative_vectors_verify_and_all_negative_cases_fail() {
    let v = vectors();
    let c = record();
    let r = registration();
    let p = vector_pair();
    assert_eq!(B64.encode(session_challenge(&r)), v["session"]["challenge"]);
    verify(&c, &r.assertion, &session_challenge(&r)).unwrap();
    let cert = &v["deviceCertificate"]["certificate"];
    let challenge = certificate_challenge(
        cert["deviceId"].as_str().unwrap(),
        cert["identityKey"].as_str().unwrap(),
        cert["pairingId"].as_str().unwrap(),
    );
    verify(&c, &p.assertion, &challenge).unwrap();
    assert_eq!(
        pairing_mac_input(cert["deviceId"].as_str().unwrap(), &p).unwrap(),
        v["pairing"]["macInput"]
    );
    assert!(valid_mac(
        &B64.decode(v["pairing"]["secret"].as_str().unwrap())
            .unwrap(),
        cert["deviceId"].as_str().unwrap(),
        &p
    ));
    let q = &v["request"];
    verify_request(
        q["sessionPublicKey"].as_str().unwrap(),
        q["aad"].as_str().unwrap(),
        &B64.decode(q["sealed"].as_str().unwrap()).unwrap(),
        q["signature"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        hash(B64.decode(&r.session_public_key).unwrap()),
        v["session"]["sessionId"]
    );
    for (name, negative) in v["negative"].as_object().unwrap() {
        let r: Registration = serde_json::from_value(negative.clone()).unwrap();
        assert!(
            verify(&c, &r.assertion, &session_challenge(&r)).is_err(),
            "{name}"
        );
    }
    let mut changed = c.clone();
    changed.rp_id = "evil.test".into();
    assert!(verify(&changed, &r.assertion, &session_challenge(&r)).is_err());
    let mut changed = r.assertion.clone();
    changed.credential_id = "other".into();
    assert!(verify(&c, &changed, &session_challenge(&r)).is_err());
    let mut changed = r.assertion.clone();
    changed.signature = B64.encode(
        Signature::from_der(&B64.decode(&changed.signature).unwrap())
            .unwrap()
            .to_bytes(),
    );
    assert!(verify(&c, &changed, &session_challenge(&r)).is_err());
    verify_request(
        q["sessionPublicKey"].as_str().unwrap(),
        "tampered-aad",
        &B64.decode(q["sealed"].as_str().unwrap()).unwrap(),
        q["signature"].as_str().unwrap(),
    )
    .unwrap_err();
    verify_request(
        q["sessionPublicKey"].as_str().unwrap(),
        q["aad"].as_str().unwrap(),
        b"tampered sealed",
        q["signature"].as_str().unwrap(),
    )
    .unwrap_err();
}
#[test]
fn assertion_requires_up_uv_type_challenge_origin_and_cross_origin_false() {
    let c = record();
    let r = registration();
    let challenge = session_challenge(&r);
    // These client-data variants are correctly signed, so failures test policy.
    for (field, value) in [
        ("type", json!("webauthn.create")),
        ("challenge", json!("wrong")),
        ("origin", json!("https://evil.test")),
        ("crossOrigin", json!(true)),
        ("crossOrigin", Value::Null),
    ] {
        let mut a = assertion(&c, &credential_key(), &challenge);
        let mut client: Value =
            serde_json::from_slice(&B64.decode(&a.client_data_json).unwrap()).unwrap();
        client[field] = value;
        let bytes = serde_json::to_vec(&client).unwrap();
        let mut input = B64.decode(&a.authenticator_data).unwrap();
        input.extend_from_slice(&Sha256::digest(&bytes));
        let sig: Signature = credential_key().sign(&input);
        a.client_data_json = B64.encode(bytes);
        a.signature = B64.encode(sig.to_der().as_bytes());
        assert!(verify(&c, &a, &challenge).is_err(), "{field}");
    }
    for flag in [0, 1, 4] {
        let mut a = assertion(&c, &credential_key(), &challenge);
        let mut auth = B64.decode(&a.authenticator_data).unwrap();
        auth[32] = flag;
        let mut input = auth.clone();
        input.extend_from_slice(&Sha256::digest(B64.decode(&a.client_data_json).unwrap()));
        let sig: Signature = credential_key().sign(&input);
        a.authenticator_data = B64.encode(auth);
        a.signature = B64.encode(sig.to_der().as_bytes());
        assert!(verify(&c, &a, &challenge).is_err());
    }
    let mut a = r.assertion.clone();
    a.authenticator_data = B64.encode([0; 36]);
    assert!(verify(&c, &a, &challenge).is_err());
}
#[test]
fn rs256_uses_spki_pkcs1_sha256() {
    use rsa::signature::SignatureEncoding;
    let key = rsa::RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    let mut c = record();
    c.algorithm = -257;
    c.public_key = B64.encode(key.to_public_key().to_public_key_der().unwrap().as_bytes());
    let challenge = b"rsa challenge";
    let mut a = assertion(&c, &credential_key(), challenge);
    let mut input = B64.decode(&a.authenticator_data).unwrap();
    input.extend_from_slice(&Sha256::digest(B64.decode(&a.client_data_json).unwrap()));
    let sig: rsa::pkcs1v15::Signature = rsa::pkcs1v15::SigningKey::<Sha256>::new(key).sign(&input);
    a.signature = B64.encode(sig.to_bytes());
    verify(&c, &a, challenge).unwrap();
    let mut sig = B64.decode(&a.signature).unwrap();
    sig[0] ^= 1;
    a.signature = B64.encode(sig);
    assert!(verify(&c, &a, challenge).is_err());
}
#[test]
fn pairing_atomic_single_use_expiry_mac_lockout_and_origin_binding() {
    let (_dir, db) = fixture();
    let p = vector_pair();
    let identity = vector_identity();
    let result = pair(&db, &identity, &p, 1).unwrap();
    assert_eq!(
        result["certificate"],
        vectors()["deviceCertificate"]["certificate"]
    );
    assert!(pair(&db, &identity, &p, 2).is_err());
    db.with(|c| {
        assert!(strict_in(c)?);
        assert!(credential(c, &p.credential.credential_id)?.is_some());
        Ok(())
    })
    .unwrap();
    let (_dir, db) = fixture();
    assert!(pair(&db, &identity, &p, 600_000).is_err());
    let mut invalid = p.clone();
    invalid.mac = B64.encode([0; 32]);
    for count in 1..=5 {
        assert!(pair(&db, &identity, &invalid, 1).is_err());
        db.with(|c| {
            assert_eq!(
                c.query_row("SELECT failures FROM pairing_offers", [], |r| r
                    .get::<_, u32>(0))?,
                count
            );
            assert!(!strict_in(c)?);
            assert!(credentials(c)?.is_empty());
            Ok(())
        })
        .unwrap();
    }
    assert!(pair(&db, &identity, &p, 1).is_err());
    for field in ["origin", "rp"] {
        let (_dir, db) = fixture();
        let mut changed = p.clone();
        if field == "origin" {
            changed.credential.origin = "https://evil.test".into();
        } else {
            changed.credential.rp_id = "evil.test".into();
        }
        mac(
            &mut changed,
            vectors()["deviceCertificate"]["certificate"]["deviceId"]
                .as_str()
                .unwrap(),
            &B64.decode(vectors()["pairing"]["secret"].as_str().unwrap())
                .unwrap(),
        );
        assert!(pair(&db, &identity, &changed, 1).is_err());
        db.with(|c| {
            assert!(!strict_in(c)?);
            assert!(credentials(c)?.is_empty());
            assert_eq!(
                c.query_row("SELECT used_at FROM pairing_offers", [], |r| r
                    .get::<_, Option<i64>>(0))?,
                None
            );
            Ok(())
        })
        .unwrap();
    }
    let (_dir, db) = fixture();
    assert!(pair(&db, "substituted identity", &p, 1).is_err());
}
#[test]
fn session_registration_lifetime_clock_skew_late_first_contact_and_persistence() {
    let (dir, db) = fixture();
    let r = registration();
    let time = r.issued_at;
    assert!(register(&db, &r, time).is_err());
    db.with(|c| store_credential(c, &record())).unwrap();
    assert_eq!(
        register(&db, &r, time).unwrap()["sessionId"],
        vectors()["session"]["sessionId"]
    );
    assert_eq!(
        register(&db, &r, time + 2 * 24 * 60 * 60 * 1000).unwrap()["sessionId"],
        vectors()["session"]["sessionId"]
    );
    assert!(register(&db, &r, time - 300_001).is_err());
    register(&db, &r, time - 300_000).unwrap();
    assert!(register(&db, &r, r.expires_at).is_err());
    for expires in [time, time - 1, time + MAX_LIFETIME + 1] {
        let mut invalid = r.clone();
        invalid.expires_at = expires;
        resign_registration(&mut invalid);
        assert!(register(&db, &invalid, time).is_err());
    }
    let mut invalid = r.clone();
    invalid.nonce = B64.encode([0; 15]);
    resign_registration(&mut invalid);
    assert!(register(&db, &invalid, time).is_err());
    let mut invalid = r.clone();
    invalid.session_public_key = B64.encode([0; 65]);
    resign_registration(&mut invalid);
    assert!(register(&db, &invalid, time).is_err());
    let id = vectors()["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(db);
    let conn = Connection::open(dir.path().join("db.sqlite")).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT credential_id FROM owner_sessions WHERE session_id=?1",
            [&id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        record().credential_id
    );
}
#[test]
fn credential_addition_last_credential_and_revocation() {
    let (_dir, db) = fixture();
    db.with(|c| store_credential(c, &record())).unwrap();
    let r = registration();
    register(&db, &r, r.issued_at).unwrap();
    let err = remove_credential(&db, &record().credential_id).unwrap_err();
    assert_eq!(
        err.downcast_ref::<Rejection>().unwrap().code,
        "last_owner_credential"
    );
    let mut new = record();
    new.credential_id = B64.encode(b"another passkey");
    let key = SigningKey::random(&mut OsRng);
    new.public_key = B64.encode(key.verifying_key().to_public_key_der().unwrap().as_bytes());
    let challenge = Sha256::digest(
        format!(
            "pockymoe-add-credential-v1\n{}\n{}\n{}",
            vectors()["deviceCertificate"]["certificate"]["deviceId"]
                .as_str()
                .unwrap(),
            new.credential_id,
            new.public_key
        )
        .as_bytes(),
    );
    let mut input = AddCredential {
        assertion: assertion(&new, &key, &challenge),
        credential: new,
    };
    add(&db, &input).unwrap();
    input.assertion.signature = B64.encode([0; 64]);
    assert!(add(&db, &input).is_err());
    remove_credential(&db, &record().credential_id).unwrap();
    assert!(session(
        &db,
        vectors()["session"]["sessionId"].as_str().unwrap(),
        r.issued_at
    )
    .unwrap()
    .is_none());
}

pub(crate) fn live_owner(state: &Supervisor) -> OwnerPrincipal {
    remember_device(state, "test-device").unwrap();
    state
        .db
        .with(|c| {
            store_credential(c, &record())?;
            c.execute("UPDATE device_owner_trust SET strict=1", [])?;
            Ok(())
        })
        .unwrap();
    let mut r = registration();
    r.issued_at = now();
    r.expires_at = r.issued_at + 3 * 24 * 60 * 60 * 1000;
    resign_registration(&mut r);
    let value = register(&state.db, &r, now()).unwrap();
    OwnerPrincipal {
        session_id: value["sessionId"].as_str().unwrap().into(),
        credential_id: record().credential_id,
    }
}
/// A browser-like HPKE request, with a response exporter, tests actual tunnel dispatch.
pub(crate) fn encrypted(
    state: &Supervisor,
    method: &str,
    path: &str,
    body: Value,
    owner: Option<&OwnerPrincipal>,
) -> (Value, [u8; 32], String) {
    use hpke::{
        aead::AesGcm256, kdf::HkdfSha256, kem::DhP256HkdfSha256 as Kem, Deserializable,
        Kem as KemTrait, Serializable,
    };
    let transport = crate::secure_transport::transport(state).unwrap();
    let d = transport.descriptor("").unwrap();
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
    if let Some(owner) = owner {
        let sig: Signature =
            session_key().sign(format!("pockymoe-request-v1\n{aad}\n{}", hash(&sealed)).as_bytes());
        request["headers"]["x-rcd-session"] = json!(owner.session_id);
        request["headers"]["x-rcd-signature"] = json!(B64.encode(sig.to_bytes()));
    }
    (request, response, aad)
}
pub(crate) fn decoded(response: Value, key: &[u8; 32], aad: &str) -> (u64, Value) {
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
    let body = serde_json::from_slice(&clear[4 + n..]).unwrap_or(Value::Null);
    (meta["status"].as_u64().unwrap(), body)
}
#[tokio::test]
async fn tunnel_strict_plain_unsigned_bad_signature_signed_and_exceptions() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    for (method, path) in [
        ("GET", "/api/version"),
        ("GET", "/api/threads/t/transport/shell-scope"),
        ("POST", "/api/transport/session"),
        ("POST", "/api/transport/pair"),
        ("GET", "/api/peer/transport/key"),
        ("POST", "/healthz"),
    ] {
        let r =
            crate::tunnel::forward_local(&state, json!({"method":method,"path":path}), None).await;
        assert_eq!(r["statusCode"], 403, "{path}");
        let body: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["code"], "device_strict_mode");
    }
    for path in [
        "/api/version",
        "/api/transport/session",
        "/api/threads/t/transport/shell-scope",
        "/api/transport/stream/no-stream",
        "/api/transport/trust",
    ] {
        let (r, key, aad) = encrypted(&state, "GET", path, Value::Null, None);
        let (status, body) = decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad,
        );
        assert_eq!(status, 401, "{path}");
        assert_eq!(body["code"], "owner_session_required");
    }
    let (mut r, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, Some(&owner));
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
    let (r, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, Some(&owner));
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        200
    );
    let (mut r, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, Some(&owner));
    r["body"] = json!(r["headers"]["x-rcd-sealed"].take());
    let sealed = B64.decode(r["body"].as_str().unwrap()).unwrap();
    r["headers"].as_object_mut().unwrap().remove("x-rcd-sealed");
    r["body"] = json!(base64::engine::general_purpose::STANDARD.encode(sealed));
    r["bodyEncoding"] = json!("base64");
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        200
    );
    let (r, key, aad) = encrypted(
        &state,
        "GET",
        "/api/transport/trust",
        Value::Null,
        Some(&owner),
    );
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["sessions"][0]["current"], true);
    let r =
        crate::tunnel::forward_local(&state, json!({"method":"GET","path":"/healthz"}), None).await;
    assert_eq!(r["statusCode"], 200);
    let r = crate::tunnel::forward_local(
        &state,
        json!({"method":"GET","path":"/api/transport/key"}),
        None,
    )
    .await;
    let body: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["trust"], json!({"paired":true,"strict":true}));
    let r = crate::tunnel::forward_local(
        &state,
        json!({"method":"GET","path":"/api/publications/missing"}),
        None,
    )
    .await;
    assert_ne!(r["statusCode"], 403);
    for path in ["/api/transport/pair", "/api/transport/owner-session"] {
        let (r, key, aad) = encrypted(&state, "POST", path, json!({}), None);
        let (status, body) = decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad,
        );
        assert_ne!(status, 401);
        assert_ne!(body["code"], "owner_session_required");
    }
    let (r, key, aad) = encrypted(
        &state,
        "DELETE",
        &format!("/api/transport/sessions/{}", owner.session_id),
        Value::Null,
        Some(&owner),
    );
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        200
    );
    assert!(!owner.valid(&state));
    let (r, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, Some(&owner));
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        401
    );
}
#[tokio::test]
async fn encrypted_pairing_and_owner_session_bootstrap_then_signed_trust() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    remember_device(&state, "test-device").unwrap();
    crate::secure_transport::transport(&state).unwrap();
    let url = create_pairing_offer(
        &state.config.database_url,
        Some("https://pockymoe.test"),
        None,
    )
    .unwrap();
    let u = url::Url::parse(&url).unwrap();
    let fragment: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(u.fragment().unwrap().as_bytes())
            .into_owned()
            .collect();
    let mut p = vector_pair();
    p.pairing_id = fragment["pairing"].clone();
    p.assertion = assertion(
        &p.credential,
        &credential_key(),
        &certificate_challenge(
            "test-device",
            &crate::secure_transport::transport(&state)
                .unwrap()
                .identity_key(),
            &p.pairing_id,
        ),
    );
    mac(
        &mut p,
        "test-device",
        &B64.decode(&fragment["secret"]).unwrap(),
    );
    let (r, key, aad) = encrypted(
        &state,
        "POST",
        "/api/transport/pair",
        serde_json::to_value(&p).unwrap(),
        None,
    );
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["paired"], true);
    assert!(strict(&state));
    let mut r = registration();
    r.issued_at = now() - 24 * 60 * 60 * 1000;
    r.expires_at = r.issued_at + 3 * 24 * 60 * 60 * 1000;
    resign_registration(&mut r);
    let (request, key, aad) = encrypted(
        &state,
        "POST",
        "/api/transport/owner-session",
        serde_json::to_value(&r).unwrap(),
        None,
    );
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, request, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["sessionId"], vectors()["session"]["sessionId"]);
}
#[tokio::test]
async fn offline_offer_and_reset_work_with_a_running_database_and_refuse_unknown_id() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    crate::secure_transport::transport(&state).unwrap();
    let err = create_pairing_offer(
        &state.config.database_url,
        Some("https://pockymoe.test"),
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("not connected"));
    let owner = live_owner(&state);
    let url = create_pairing_offer(
        &state.config.database_url,
        None,
        Some("wss://pockymoe.test/tunnel"),
    )
    .unwrap();
    let url = url::Url::parse(&url).unwrap();
    assert_eq!(url.origin().ascii_serialization(), "https://pockymoe.test");
    assert_eq!(url.path(), "/pair");
    assert!(url.query().is_none());
    for invalid in [
        "https://user:password@pockymoe.test",
        "https://pockymoe.test/path",
        "https://pockymoe.test?query=yes",
    ] {
        assert!(create_pairing_offer(&state.config.database_url, Some(invalid), None).is_err());
    }
    state
        .db
        .with(|c| {
            c.execute("INSERT INTO device_certificates VALUES(1,'{}')", [])?;
            Ok(())
        })
        .unwrap();
    reset_owner_trust(&state.config.database_url).unwrap();
    assert!(!strict(&state));
    assert!(!owner.valid(&state));
    state
        .db
        .with(|c| {
            for table in [
                "owner_credentials",
                "owner_sessions",
                "device_certificates",
                "pairing_offers",
                "owner_port_mappings",
            ] {
                assert_eq!(
                    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, u32>(0))?,
                    0
                );
            }
            assert_eq!(device_id(c)?, "test-device");
            Ok(())
        })
        .unwrap();
    let r =
        crate::tunnel::forward_local(&state, json!({"method":"GET","path":"/api/version"}), None)
            .await;
    assert_eq!(r["statusCode"], 200);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn signed_file_save_uses_owner_actor_despite_forged_relay_actor() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let workspace = state
        .create_workspace(pockymoe_protocol::CreateWorkspaceInput {
            abs_path: Some(state.config.workspace_root.to_string_lossy().into()),
            git_url: None,
            label: Some("files".into()),
        })
        .unwrap();
    std::fs::write(state.config.workspace_root.join("a.txt"), "base").unwrap();
    let doc = serde_json::to_value(state.file_document(&workspace.id, "a.txt").unwrap()).unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let input = json!({"path":"a.txt","workspaceRevision":doc["workspaceRevision"],"fileIdentity":doc["fileIdentity"],"expectedHash":doc["contentHash"],"content":"owner save","draftRevision":1,"operationId":operation,"operationCreatedAt":now()});
    let (mut r, key, aad) = encrypted(
        &state,
        "POST",
        &format!("/api/workspaces/{}/files/save", workspace.id),
        input,
        Some(&owner),
    );
    r["headers"]["x-rcd-file-actor"] = json!("forged");
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["status"], "saved");
    state
        .db
        .with(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT actor FROM file_save_operations WHERE operation_id=?1",
                    [&operation],
                    |r| r.get::<_, String>(0)
                )?,
                owner.actor()
            );
            Ok(())
        })
        .unwrap();
    let (mut r, key, aad) = encrypted(
        &state,
        "GET",
        &format!(
            "/api/workspaces/{}/files/operations/{operation}",
            workspace.id
        ),
        Value::Null,
        Some(&owner),
    );
    r["headers"]["x-rcd-file-actor"] = json!("different forged actor");
    assert_eq!(
        decoded(
            crate::tunnel::forward_local(&state, r, None).await,
            &key,
            &aad
        )
        .0,
        200
    );
}

#[tokio::test]
async fn owner_endpoints_refuse_plaintext_even_with_local_login_or_forged_headers() {
    use tower::ServiceExt;
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let token = crate::auth::login(&state.config, "admin", "secret123")
        .unwrap()
        .0;
    for path in [
        "/api/transport/pair",
        "/api/transport/owner-session",
        "/api/transport/trust",
        "/api/transport/credentials",
        "/api/transport/credentials/one",
        "/api/transport/sessions/one",
    ] {
        let r = crate::http::router(state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .method(if path.ends_with("trust") {
                        "GET"
                    } else {
                        "POST"
                    })
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-rcd-session", "forged")
                    .header("x-rcd-signature", "forged")
                    .header("x-pockymoe-relay-forwarded", "1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn signed_credential_management_checks_new_proof_last_owner_and_session_revocation() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    let owner = live_owner(&state);
    let path = format!("/api/transport/credentials/{}", owner.credential_id);
    let (r, key, aad) = encrypted(&state, "DELETE", &path, Value::Null, Some(&owner));
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 409);
    assert_eq!(body["code"], "last_owner_credential");
    let mut new = record();
    new.credential_id = B64.encode(b"new owner credential");
    let keypair = SigningKey::random(&mut OsRng);
    new.public_key = B64.encode(
        keypair
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    );
    let challenge = Sha256::digest(
        format!(
            "pockymoe-add-credential-v1\ntest-device\n{}\n{}",
            new.credential_id, new.public_key
        )
        .as_bytes(),
    );
    let body = json!({"credential":new,"assertion":assertion(&new,&keypair,&challenge)});
    let (r, key, aad) = encrypted(
        &state,
        "POST",
        "/api/transport/credentials",
        body,
        Some(&owner),
    );
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["credentials"].as_array().unwrap().len(), 2);
    let (r, key, aad) = encrypted(&state, "DELETE", &path, Value::Null, Some(&owner));
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, r, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 200);
    assert_eq!(body["credentials"].as_array().unwrap().len(), 1);
    assert!(!owner.valid(&state));
}

#[tokio::test]
async fn unpaired_ignores_owner_headers_while_paired_rejects_unknown_and_expired_sessions() {
    let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
    for headers in [
        json!({"x-rcd-session":"unknown","x-rcd-signature":"invalid"}),
        json!({"x-rcd-session":"unknown"}),
        json!({"x-rcd-signature":"invalid"}),
    ] {
        let response = crate::tunnel::forward_local(
            &state,
            json!({"method":"GET","path":"/api/version","headers":headers}),
            None,
        )
        .await;
        assert_eq!(response["statusCode"], 200);
        let (mut request, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, None);
        for (name, value) in headers.as_object().unwrap() {
            request["headers"][name] = value.clone();
        }
        assert_eq!(
            decoded(
                crate::tunnel::forward_local(&state, request, None).await,
                &key,
                &aad
            )
            .0,
            200
        );
    }
    let owner = live_owner(&state);
    let (mut request, key, aad) =
        encrypted(&state, "GET", "/api/version", Value::Null, Some(&owner));
    request["headers"]["x-rcd-session"] = json!("unknown");
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, request, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 401);
    assert_eq!(body["code"], "owner_session_required");
    state
        .db
        .with(|c| {
            c.execute("UPDATE owner_sessions SET expires_at=0", [])?;
            Ok(())
        })
        .unwrap();
    let (request, key, aad) = encrypted(&state, "GET", "/api/version", Value::Null, Some(&owner));
    let (status, body) = decoded(
        crate::tunnel::forward_local(&state, request, None).await,
        &key,
        &aad,
    );
    assert_eq!(status, 401);
    assert_eq!(body["code"], "owner_session_required");
}
