//! Account-key peer authority through the encrypted in-process HTTP dispatcher.
use super::tests::{
    account_key, bound_peer, bound_peer_with_account, decoded, device_revocation, encrypted,
    vectors,
};
use super::*;
use crate::{
    auth::PeerCaller,
    tunnel::{forward_local, tests::state_with_relay_url},
};
use p256::ecdsa::{signature::Signer, SigningKey};
use rand::rngs::OsRng;

fn caller(id: &str) -> PeerCaller {
    PeerCaller {
        device_id: id.into(),
        device_name: "Test peer".into(),
        user_id: "test-owner".into(),
        shared_control: None,
    }
}
async fn register(target: &Arc<Supervisor>, certificate: Value, device: &str) -> (u64, Value) {
    let (request, key, aad) = encrypted(
        target,
        "POST",
        "/api/peer/transport/certificate",
        certificate,
        None,
    );
    decoded(
        forward_local(target, request, Some(caller(device))).await,
        &key,
        &aad,
    )
}
fn signed(target: &Supervisor, source: &Supervisor, device: &str) -> (Value, [u8; 32], String) {
    let (mut request, key, aad) = encrypted(
        target,
        "POST",
        "/api/peer/cli",
        json!({"operation":"info"}),
        None,
    );
    // Independent signing-input construction, using only a temporary test identity.
    let identity = SigningKey::from_slice(
        &B64.decode(
            std::fs::read_to_string(
                source
                    .config
                    .database_url
                    .with_extension("transport-identity"),
            )
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let sealed = B64
        .decode(request["headers"]["x-rcd-sealed"].as_str().unwrap())
        .unwrap();
    let signature: Signature = identity.sign(
        format!(
            "pockymoe-peer-request-v1\n{device}\n{aad}\n{}",
            hash(&sealed)
        )
        .as_bytes(),
    );
    request["headers"]["x-rcd-peer-device"] = json!(device);
    request["headers"]["x-rcd-peer-signature"] = json!(B64.encode(signature.to_bytes()));
    (request, key, aad)
}
fn registrations(state: &Supervisor) -> i64 {
    state
        .db
        .with(|c| Ok(c.query_row("SELECT count(*) FROM peer_certificates", [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn peer_signature_vector_binds_caller_aad_and_exact_ciphertext() {
    let (_dir, target) = state_with_relay_url("http://127.0.0.1:1");
    bound_peer(&target, "target");
    let v = vectors();
    let certificate: DeviceCertificate =
        serde_json::from_value(v["deviceCertificate"]["certificate"].clone()).unwrap();
    let peer = &v["peerSignature"];
    let device = peer["callerDeviceId"].as_str().unwrap();
    let aad = peer["aad"].as_str().unwrap();
    let sealed = B64.decode(peer["sealed"].as_str().unwrap()).unwrap();
    let headers =
        json!({"headers":{"x-rcd-peer-device":device,"x-rcd-peer-signature":peer["signature"]}});
    target.db.with(|c| {
        c.execute("INSERT INTO peer_certificates(device_id,identity_key,account_key_id) VALUES(?1,?2,?3)",params![certificate.device_id, certificate.identity_key, certificate.account_key_id])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        peer_signing_input(device, aad, &sealed),
        peer["signingInput"]
    );
    let principal = authorize_peer(&target, &caller(device), &headers, aad, &sealed).unwrap();
    assert!(principal.valid(&target, device));
    assert!(authorize_peer(&target, &caller(device), &headers, "modified aad", &sealed).is_err());
    assert!(authorize_peer(
        &target,
        &caller(device),
        &headers,
        aad,
        b"modified ciphertext"
    )
    .is_err());
    assert!(authorize_peer(&target, &caller("other-device"), &headers, aad, &sealed).is_err());
    let mut bad = headers.clone();
    bad["headers"]["x-rcd-peer-signature"] = json!(B64.encode([0; 64]));
    assert!(authorize_peer(&target, &caller(device), &bad, aad, &sealed).is_err());
    apply_revocations(&target, &[device_revocation(device)]).unwrap();
    assert_eq!(registrations(&target), 0);
    assert!(!principal.valid(&target, device));
    assert!(authorize_peer(&target, &caller(device), &headers, aad, &sealed).is_err());
}

#[tokio::test]
async fn registration_checks_account_device_identity_signature_and_opt_in() {
    let (_source_dir, source) = state_with_relay_url("http://127.0.0.1:1");
    let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
    let cert = bound_peer(&source, "caller");
    bound_peer(&target, "target");
    let (request, _, _) = encrypted(
        &target,
        "POST",
        "/api/peer/transport/certificate",
        json!(cert),
        None,
    );
    let response = forward_local(&target, request, Some(caller("caller"))).await;
    assert_eq!(response["statusCode"], 403);
    assert_eq!(
        serde_json::from_str::<Value>(response["body"].as_str().unwrap()).unwrap()["code"],
        "peer_access_disabled"
    );
    target.set_peer_access(true).unwrap();
    let (_other_dir, other) = state_with_relay_url("http://127.0.0.1:1");
    let other_cert = bound_peer_with_account(&other, "caller", &SigningKey::random(&mut OsRng));
    for invalid in [
        json!(other_cert),
        {
            let mut v = json!(cert);
            v["deviceId"] = json!("forged");
            v
        },
        {
            let mut v = json!(cert);
            v["identityKey"] = vectors()["deviceCertificate"]["certificate"]["identityKey"].clone();
            v
        },
        {
            let mut v = json!(cert);
            v["signature"] = json!(B64.encode([0; 64]));
            v
        },
        {
            let mut v = json!(cert);
            v["version"] = json!(2);
            v
        },
    ] {
        let (status, body) = register(&target, invalid, "caller").await;
        assert_eq!(status, 401);
        assert_eq!(body["code"], "peer_certificate_required");
        assert_eq!(registrations(&target), 0);
    }
    assert_eq!(register(&target, json!(cert), "wrong-caller").await.0, 401);
    assert_eq!(register(&target, json!(cert), "caller").await.0, 200);
    assert_eq!(register(&target, json!(cert), "caller").await.0, 200);
    assert_eq!(registrations(&target), 1);
    let saved = target
        .db
        .with(|c| {
            Ok(c.query_row(
                "SELECT device_id,identity_key,account_key_id FROM peer_certificates",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )?)
        })
        .unwrap();
    assert_eq!(
        saved,
        (cert.device_id, cert.identity_key, cert.account_key_id)
    );
}

#[tokio::test]
async fn registered_peer_requests_reject_forged_headers_replays_and_revocations() {
    let (_source_dir, source) = state_with_relay_url("http://127.0.0.1:1");
    let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
    let certificate = bound_peer(&source, "caller");
    bound_peer(&target, "target");
    target.set_peer_access(true).unwrap();
    assert_eq!(register(&target, json!(certificate), "caller").await.0, 200);
    for variant in 0..5 {
        let (mut request, key, aad) = signed(&target, &source, "caller");
        match variant {
            0 => {
                request["headers"]["x-rcd-peer-device"] = json!("forged");
            }
            1 => {
                request["headers"]["x-rcd-peer-signature"] = json!(B64.encode([0; 64]));
            }
            2 => {
                request["headers"]
                    .as_object_mut()
                    .unwrap()
                    .remove("x-rcd-peer-signature");
            }
            3 => {
                request["headers"]["x-rcd-peer-signature"] = json!("invalid-base64!");
            }
            _ => {}
        }
        let (status, body) = decoded(
            forward_local(&target, request.clone(), Some(caller("caller"))).await,
            &key,
            &aad,
        );
        assert_eq!(status, if variant == 4 { 200 } else { 401 });
        if variant != 4 {
            assert_eq!(body["code"], "peer_certificate_required");
        }
        assert_eq!(
            forward_local(&target, request, Some(caller("caller"))).await["statusCode"],
            400
        );
    }
    let (request, key, aad) = signed(&target, &source, "caller");
    // Deliver through the actual encrypted revocation endpoint, without a principal.
    let revocation = device_revocation("caller");
    let (revoke, revoke_key, revoke_aad) = encrypted(
        &target,
        "PUT",
        "/api/transport/revocations",
        json!({"revocations":[revocation]}),
        None,
    );
    assert_eq!(
        decoded(
            forward_local(&target, revoke, None).await,
            &revoke_key,
            &revoke_aad
        )
        .0,
        200
    );
    assert_eq!(registrations(&target), 0);
    assert_eq!(
        decoded(
            forward_local(&target, request, Some(caller("caller"))).await,
            &key,
            &aad
        )
        .0,
        401
    );
    assert_eq!(register(&target, json!(certificate), "caller").await.0, 401);
    assert_eq!(registrations(&target), 0);
    // Tombstones survive a fresh database connection and cannot be undone by a newer certificate.
    let reopened = Connection::open_with_flags(
        &target.config.database_url,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert!(revoked(&reopened, &certificate.account_key_id, "device", "caller").unwrap());
    let mut new_cert = certificate;
    new_cert.created_at = now() + 1;
    let signature: Signature = account_key().sign(device_signing_input(&new_cert).as_bytes());
    new_cert.signature = B64.encode(signature.to_bytes());
    assert_eq!(register(&target, json!(new_cert), "caller").await.0, 401);
}

#[tokio::test]
async fn unbound_target_ignores_peer_headers_and_reset_drops_registrations() {
    let (_source_dir, source) = state_with_relay_url("http://127.0.0.1:1");
    let (_target_dir, target) = state_with_relay_url("http://127.0.0.1:1");
    let cert = bound_peer(&source, "caller");
    bound_peer(&target, "target");
    target.set_peer_access(true).unwrap();
    assert_eq!(register(&target, json!(cert), "caller").await.0, 200);
    reset_account_trust(&target.config.database_url).unwrap();
    assert_eq!(registrations(&target), 0);
    assert_eq!(
        register(&target, json!({"untrusted":"ignored"}), "caller")
            .await
            .0,
        200
    );
    assert_eq!(registrations(&target), 0);
    let (mut request, key, aad) = encrypted(
        &target,
        "POST",
        "/api/peer/cli",
        json!({"operation":"info"}),
        None,
    );
    request["headers"]["x-rcd-peer-device"] = json!("forged");
    request["headers"]["x-rcd-peer-signature"] = json!("malformed");
    assert_eq!(
        decoded(
            forward_local(&target, request, Some(caller("caller"))).await,
            &key,
            &aad
        )
        .0,
        200
    );
}
