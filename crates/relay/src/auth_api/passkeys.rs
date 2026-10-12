use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use webauthn_rs::prelude::*;

fn webauthn(state: &AppState) -> Result<Webauthn, Failure> {
    let (rp, origin) = rp_origin(state)?;
    WebauthnBuilder::new(&rp, &origin)
        .and_then(|b| b.rp_name("Pockymoe").build())
        .map_err(internal)
}

fn rp_origin(state: &AppState) -> Result<(String, Url), Failure> {
    let base = state
        .oauth
        .public_base_url
        .as_deref()
        .ok_or_else(|| invalid("Passkeys require a configured public URL"))?;
    let origin = Url::parse(base).map_err(internal)?;
    let rp = origin
        .host_str()
        .ok_or_else(|| invalid("Invalid public URL"))?;
    Ok((rp.to_string(), origin))
}
pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/relay/account/passkeys/public", get(public))
        .route(
            "/relay/account/security/passkeys/register/start",
            post(register_start),
        )
        .route(
            "/relay/account/security/passkeys/register/finish",
            post(register_finish),
        )
        .route(
            "/relay/account/security/passkeys/{id}",
            axum::routing::patch(rename).delete(remove),
        )
        .route("/relay/auth/passkey/start", post(authenticate_start))
        .route("/relay/auth/passkey/finish", post(authenticate_finish))
}

fn public_key_spki(key: &COSEKey) -> Result<String, Failure> {
    match (&key.type_, &key.key) {
        (COSEAlgorithm::ES256, COSEKeyType::EC_EC2(ec)) if ec.curve == ECDSACurve::SECP256R1 => {}
        (COSEAlgorithm::RS256, COSEKeyType::RSA(_)) => {}
        _ => return Err(internal("Unsupported owner credential algorithm")),
    }
    let der = key
        .get_openssl_pkey()
        .map_err(internal)?
        .public_key_to_der()
        .map_err(internal)?;
    Ok(URL_SAFE_NO_PAD.encode(der))
}

async fn public(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, _) = auth(&conn, &state, &headers)?;
    let (rp, origin) = rp_origin(&state)?;
    let origin = origin.origin().ascii_serialization();
    let mut stmt = conn
        .prepare("SELECT credential,name FROM relay_passkeys WHERE user_id=?1 ORDER BY created_at DESC,id")
        .map_err(internal)?;
    let rows = stmt
        .query_map(params![user.id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(internal)?;
    let mut passkeys = Vec::new();
    for row in rows {
        let (raw, name) = row.map_err(internal)?;
        let key: Passkey = serde_json::from_str(&raw).map_err(internal)?;
        passkeys.push(json!({
            "credentialId": URL_SAFE_NO_PAD.encode(key.cred_id().as_ref()),
            "publicKey": public_key_spki(key.get_public_key())?,
            "algorithm": *key.cred_algorithm() as i32,
            "rpId": rp,
            "origin": origin,
            "name": name,
        }));
    }
    Ok(Json(json!({"passkeys": passkeys})))
}
fn keys(conn: &Connection, user: &str) -> Result<Vec<Passkey>, Failure> {
    let mut stmt = conn
        .prepare("SELECT credential FROM relay_passkeys WHERE user_id=?1")
        .map_err(internal)?;
    let rows = stmt
        .query_map(params![user], |r| r.get::<_, String>(0))
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    rows.into_iter()
        .map(|r| serde_json::from_str(&r).map_err(internal))
        .collect()
}
#[derive(Deserialize)]
struct NameInput {
    name: String,
}
fn name(value: &str) -> Result<String, Failure> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 {
        return Err(invalid("Use a name between 1 and 80 characters"));
    }
    Ok(value.to_string())
}
async fn register_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<NameInput>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    let keys = keys(&conn, &user.id)?;
    if keys.len() >= 20 {
        return Err(invalid("Remove an unused passkey first"));
    }
    let uuid = Uuid::from_slice(&Sha256::digest(user.id.as_bytes())[..16]).map_err(internal)?;
    let (options, registration) = webauthn(&state)?
        .start_passkey_registration(
            uuid,
            &user.username,
            &user.username,
            Some(keys.iter().map(|k| k.cred_id().clone()).collect()),
        )
        .map_err(internal)?;
    let id = factors::challenge(
        &conn,
        &user.id,
        "passkey-register",
        &headers,
        Some(&token),
        json!({"state":registration,"name":name(&body.name)?}),
    )
    .map_err(internal)?;
    Ok(Json(json!({"challengeId":id,"options":options})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterInput {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
}
fn consume(
    conn: &Connection,
    id: &str,
    purpose: &str,
    headers: &HeaderMap,
    user: &str,
    session: Option<&str>,
) -> Result<Value, Failure> {
    let raw:String=conn.query_row("DELETE FROM relay_factor_challenges WHERE id_hash=?1 AND user_id=?2 AND purpose=?3 AND environment=?4 AND expires_at>?5 AND session_hash IS ?6 RETURNING data",params![security::token_hash(id),user,purpose,factors::environment(headers),factors::now(),session.map(security::token_hash)],|r|r.get(0)).map_err(|_|invalid("Verification expired. Please try again."))?;
    serde_json::from_str(&raw).map_err(internal)
}
async fn register_finish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterInput>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    let data = consume(
        &conn,
        &body.challenge_id,
        "passkey-register",
        &headers,
        &user.id,
        Some(&token),
    )?;
    let registration: PasskeyRegistration =
        serde_json::from_value(data["state"].clone()).map_err(internal)?;
    let key = webauthn(&state)?
        .finish_passkey_registration(&body.credential, &registration)
        .map_err(|_| invalid("Passkey registration could not be verified"))?;
    let id = URL_SAFE_NO_PAD.encode(key.cred_id().as_ref());
    // Global primary key prevents one credential from being assigned to two users.
    conn.execute(
        "INSERT INTO relay_passkeys(id,user_id,credential,name,created_at) VALUES (?1,?2,?3,?4,?5)",
        params![
            id,
            user.id,
            serde_json::to_string(&key).map_err(internal)?,
            data["name"].as_str().unwrap_or("Passkey"),
            factors::now()
        ],
    )
    .map_err(|_| invalid("This passkey is already registered"))?;
    factors::mark_strong(&conn, &token).map_err(internal)?;
    security::audit(&conn, Some(&user.id), "passkey.added", None);
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash<>?2",
        params![user.id, security::token_hash(&token)],
    )
    .map_err(internal)?;
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM relay_recovery_codes WHERE user_id=?1",
            params![user.id],
            |r| r.get(0),
        )
        .map_err(internal)?;
    let recovery = if count == 0 {
        Some(factors::recovery_codes(&conn, &user.id).map_err(internal)?)
    } else {
        None
    };
    Ok(Json(json!({"ok":true,"recoveryCodes":recovery})))
}
#[derive(Deserialize)]
struct StartInput {
    purpose: String,
}
async fn authenticate_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<StartInput>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, session, login_challenge) = if body.purpose == "login" {
        let (challenge, user) = pending(&conn, &headers, true)?;
        (user, None, Some(challenge))
    } else if body.purpose == "reauth" {
        let (user, token) = auth(&conn, &state, &headers)?;
        (user.id, Some(token), None)
    } else {
        return Err(invalid("Invalid verification purpose"));
    };
    if !state.admission.allow(format!("passkey:{user}"), 15, 300) {
        return Err(failure(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many attempts. Try again later.",
        ));
    }
    let keys = keys(&conn, &user)?;
    let (options, authentication) = webauthn(&state)?
        .start_passkey_authentication(&keys)
        .map_err(|_| invalid("No passkey is available for this account"))?;
    let id = factors::challenge(
        &conn,
        &user,
        &format!("passkey-{}", body.purpose),
        &headers,
        session.as_deref(),
        json!({"state":authentication,"loginChallenge":login_challenge}),
    )
    .map_err(internal)?;
    Ok(Json(json!({"challengeId":id,"options":options})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticateInput {
    challenge_id: String,
    purpose: String,
    credential: PublicKeyCredential,
    #[serde(default)]
    remember_browser: bool,
}
async fn authenticate_finish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AuthenticateInput>,
) -> Result<Response, Failure> {
    let conn = state.store.conn.lock().await;
    let (user, session, login_challenge) = if body.purpose == "login" {
        let (challenge, user) = pending(&conn, &headers, false)?;
        (user, None, Some(challenge))
    } else if body.purpose == "reauth" {
        let (user, token) = auth(&conn, &state, &headers)?;
        (user.id, Some(token), None)
    } else {
        return Err(invalid("Invalid verification purpose"));
    };
    let data = consume(
        &conn,
        &body.challenge_id,
        &format!("passkey-{}", body.purpose),
        &headers,
        &user,
        session.as_deref(),
    )?;
    if data["loginChallenge"].as_str() != login_challenge.as_deref() {
        return Err(invalid("Login verification changed. Try again."));
    }
    let authentication: PasskeyAuthentication =
        serde_json::from_value(data["state"].clone()).map_err(internal)?;
    let result = webauthn(&state)?
        .finish_passkey_authentication(&body.credential, &authentication)
        .map_err(|_| invalid("Passkey could not be verified"))?;
    let mut matched = false;
    for mut key in keys(&conn, &user)? {
        if key.update_credential(&result).is_some() {
            conn.execute("UPDATE relay_passkeys SET credential=?2,last_used_at=?3 WHERE id=?1 AND user_id=?4",params![URL_SAFE_NO_PAD.encode(key.cred_id().as_ref()),serde_json::to_string(&key).map_err(internal)?,factors::now(),user]).map_err(internal)?;
            matched = true;
        }
    }
    if !matched {
        return Err(invalid(
            "Passkey was removed. Try another verification method.",
        ));
    }
    if let Some(token) = session {
        factors::mark_strong(&conn, &token).map_err(internal)?;
        let proof = factors::password_grant(&conn, &user, &token, &headers).map_err(internal)?;
        Ok(Json(json!({"ok":true,"verificationToken":proof})).into_response())
    } else {
        conn.execute(
            "DELETE FROM relay_factor_challenges WHERE id_hash=?1",
            params![login_challenge],
        )
        .map_err(internal)?;
        let user = super::super::load_user_by_id(&conn, &user)
            .ok_or_else(|| invalid("Account unavailable"))?;
        login_response(&conn, &state, &user, &headers, true, body.remember_browser)
    }
}
async fn rename(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<NameInput>,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    conn.execute(
        "UPDATE relay_passkeys SET name=?3 WHERE id=?1 AND user_id=?2",
        params![id, user.id, name(&body.name)?],
    )
    .map_err(internal)?;
    Ok(Json(json!({"ok":true})))
}
async fn remove(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult {
    let conn = state.store.conn.lock().await;
    let (user, token) = auth(&conn, &state, &headers)?;
    recent(&conn, &token)?;
    if !factors::has_totp(&conn, &user.id) && keys(&conn, &user.id)?.len() <= 1 {
        return Err(invalid(
            "Add another passkey or authenticator before removing your last passkey",
        ));
    }
    conn.execute(
        "DELETE FROM relay_passkeys WHERE id=?1 AND user_id=?2",
        params![id, user.id],
    )
    .map_err(internal)?;
    conn.execute(
        "DELETE FROM relay_auth_sessions WHERE user_id=?1 AND token_hash<>?2",
        params![user.id, security::token_hash(&token)],
    )
    .map_err(internal)?;
    security::audit(&conn, Some(&user.id), "passkey.removed", None);
    conn.execute(
        "DELETE FROM relay_trusted_browsers WHERE user_id=?1",
        params![user.id],
    )
    .map_err(internal)?;
    conn.execute(
        "DELETE FROM relay_factor_challenges WHERE user_id=?1",
        params![user.id],
    )
    .map_err(internal)?;
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::{
        bn::{BigNum, BigNumContext},
        ec::{EcGroup, EcKey},
        nid::Nid,
        pkey::{HasPublic, PKey},
    };

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../test-fixtures/owner-trust-v1.json")).unwrap()
    }

    fn cose_from_ec<T: HasPublic>(key: &EcKey<T>) -> COSEKey {
        let mut ctx = BigNumContext::new().unwrap();
        let mut x = BigNum::new().unwrap();
        let mut y = BigNum::new().unwrap();
        key.public_key()
            .affine_coordinates_gfp(key.group(), &mut x, &mut y, &mut ctx)
            .unwrap();
        COSEKey {
            type_: COSEAlgorithm::ES256,
            key: COSEKeyType::EC_EC2(COSEEC2Key {
                curve: ECDSACurve::SECP256R1,
                x: x.to_vec_padded(32).unwrap().into(),
                y: y.to_vec_padded(32).unwrap().into(),
            }),
        }
    }

    fn vector_key() -> COSEKey {
        let der = URL_SAFE_NO_PAD
            .decode(vectors()["credentialRecord"]["publicKey"].as_str().unwrap())
            .unwrap();
        let key = PKey::public_key_from_der(&der).unwrap().ec_key().unwrap();
        cose_from_ec(&key)
    }

    #[test]
    fn es256_cose_spki_round_trips_and_matches_interop_vector() {
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let key = EcKey::generate(&group).unwrap();
        let cose = cose_from_ec(&key);
        let der = URL_SAFE_NO_PAD
            .decode(public_key_spki(&cose).unwrap())
            .unwrap();
        let decoded = PKey::public_key_from_der(&der).unwrap().ec_key().unwrap();
        assert_eq!(cose_from_ec(&decoded), cose);
        assert_eq!(
            der,
            PKey::from_ec_key(key).unwrap().public_key_to_der().unwrap()
        );
        assert_eq!(
            public_key_spki(&vector_key()).unwrap(),
            vectors()["credentialRecord"]["publicKey"]
        );
    }

    #[test]
    fn rs256_cose_exports_spki_and_invalid_keys_are_rejected() {
        let rsa = openssl::rsa::Rsa::generate(2048).unwrap();
        let cose = COSEKey {
            type_: COSEAlgorithm::RS256,
            key: COSEKeyType::RSA(COSERSAKey {
                n: rsa.n().to_vec().into(),
                e: rsa.e().to_vec().try_into().unwrap(),
            }),
        };
        let expected = PKey::from_rsa(rsa).unwrap().public_key_to_der().unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(public_key_spki(&cose).unwrap())
                .unwrap(),
            expected
        );
        let mut wrong_algorithm = vector_key();
        wrong_algorithm.type_ = COSEAlgorithm::RS256;
        assert!(public_key_spki(&wrong_algorithm).is_err());
        let mut invalid = vector_key();
        if let COSEKeyType::EC_EC2(ref mut ec) = invalid.key {
            ec.x = vec![0; 32].into();
            ec.y = vec![0; 32].into();
        }
        assert!(public_key_spki(&invalid).is_err());
    }

    #[tokio::test]
    async fn public_directory_uses_account_credential_and_configured_rp_origin() {
        let fixture = crate::trust_tests::Fixture::new("passkey-public-directory").await;
        let record = vectors()["credentialRecord"].clone();
        let passkey: Passkey = serde_json::from_value(json!({"cred": {
            "cred_id": record["credentialId"], "cred": vector_key(), "counter": 0,
            "transports": null, "user_verified": true, "backup_eligible": true, "backup_state": true,
            "registration_policy": "required", "extensions": {},
            "attestation": { "data": "None", "metadata": "None" }, "attestation_format": "none"
        }})).unwrap();
        {
            let conn = fixture.state.store.conn.lock().await;
            conn.execute(
                "INSERT INTO relay_passkeys(id,user_id,credential,name,created_at)
                          VALUES (?1,'owner',?2,'Owner passkey',0)",
                params![
                    record["credentialId"].as_str().unwrap(),
                    serde_json::to_string(&passkey).unwrap()
                ],
            )
            .unwrap();
        }
        let path = "/relay/account/passkeys/public";
        assert_eq!(
            fixture
                .request(axum::http::Method::GET, path, None)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let other: Value = fixture
            .request(axum::http::Method::GET, path, Some("stranger"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(other, json!({"passkeys": []}));
        let response = fixture
            .request(axum::http::Method::GET, path, Some("owner"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            body,
            json!({"passkeys": [{
                "credentialId": record["credentialId"], "publicKey": record["publicKey"], "algorithm": -7,
                "rpId": "pockymoe.test", "origin": "https://pockymoe.test:8443", "name": "Owner passkey"
            }]})
        );
        // Public records have no registration metadata, counters, or private material.
        assert_eq!(body["passkeys"][0].as_object().unwrap().len(), 6);
        webauthn(&fixture.state).unwrap();
    }
}
