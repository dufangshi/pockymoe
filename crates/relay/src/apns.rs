//! Native Apple delivery reuses the authenticated, durable notification outbox.
use super::*;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openssl::{ecdsa::EcdsaSig, hash::MessageDigest, pkey::PKey, sign::Signer};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Subscription {
    pub native_platform: String,
    #[serde(default)]
    pub app_kind: AppKind,
    pub device_token: String,
    pub relay_origin: String,
    #[serde(default)]
    pub sandbox: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(super) enum AppKind {
    #[default]
    #[serde(rename = "ios")]
    Ios,
    #[serde(rename = "macos-desktop")]
    MacosDesktop,
}

impl AppKind {
    fn topic_env(self) -> &'static str {
        match self {
            Self::Ios => "POCKYMOE_APNS_TOPIC",
            Self::MacosDesktop => "POCKYMOE_APNS_DESKTOP_TOPIC",
        }
    }
}

pub(super) fn configured() -> bool {
    configured_for(AppKind::Ios)
}

pub(super) fn configured_for(kind: AppKind) -> bool {
    [
        "POCKYMOE_APNS_KEY_PATH",
        "POCKYMOE_APNS_KEY_ID",
        "POCKYMOE_APNS_TEAM_ID",
        kind.topic_env(),
    ]
    .iter()
    .all(|key| std::env::var(key).is_ok_and(|v| !v.trim().is_empty()))
}

pub(super) fn configured_kinds() -> Vec<AppKind> {
    [AppKind::Ios, AppKind::MacosDesktop]
        .into_iter()
        .filter(|kind| configured_for(*kind))
        .collect()
}

pub(super) fn subscription_id(sub: &Subscription) -> String {
    security::token_hash(&format!(
        "apns:{}:{}:{}",
        serde_json::to_string(&sub.app_kind).expect("app kind"),
        sub.sandbox,
        sub.device_token.to_ascii_lowercase()
    ))
}

fn collapse_id(tag: &str) -> String {
    if tag.len() <= 64 && tag.bytes().all(|b| (32..127).contains(&b)) {
        tag.into()
    } else {
        security::token_hash(tag)
    }
}

fn native_payload(event: &Value, sub: &Subscription) -> Result<Value> {
    let parts: Vec<&str> = event["url"].as_str().unwrap_or("").split('/').collect();
    anyhow::ensure!(
        parts.len() == 5 && parts[1] == "devices" && parts[3] == "threads",
        "Invalid notification route"
    );
    Ok(json!({
        "aps":{"alert":{"title":event["title"],"body":event["body"]},"sound":"default","thread-id":format!("{}:{}",parts[2],parts[4])},
        "deviceId":parts[2],"threadId":parts[4],"eventId":event["tag"],
        "tag":event["tag"],"url":event["url"],"relayOrigin":sub.relay_origin,
    }))
}

pub(super) fn valid_token(token: &str) -> bool {
    (64..=200).contains(&token.len())
        && token.len() % 2 == 0
        && token.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn validate(sub: &Subscription) -> Result<()> {
    anyhow::ensure!(sub.native_platform == "apns", "Unsupported native platform");
    let origin = reqwest::Url::parse(&sub.relay_origin)?;
    anyhow::ensure!(
        matches!(origin.scheme(), "https" | "http")
            && origin.host_str().is_some()
            && origin.username().is_empty()
            && origin.password().is_none()
            && origin.path() == "/"
            && origin.query().is_none()
            && origin.fragment().is_none(),
        "Invalid relay origin"
    );
    anyhow::ensure!(valid_token(&sub.device_token), "Invalid APNs token");
    Ok(())
}

fn jwt(pem: &[u8], key_id: &str, team_id: &str, now: i64) -> Result<String> {
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"alg":"ES256","kid":key_id}))?);
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"iss":team_id,"iat":now}))?);
    let signing = format!("{header}.{claims}");
    let key = PKey::private_key_from_pem(pem)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
    signer.update(signing.as_bytes())?;
    let der = signer.sign_to_vec()?;
    let signature = EcdsaSig::from_der(&der)?;
    let mut raw = signature.r().to_vec_padded(32)?;
    raw.extend(signature.s().to_vec_padded(32)?);
    Ok(format!("{signing}.{}", URL_SAFE_NO_PAD.encode(raw)))
}

pub(super) async fn deliver(
    http: &reqwest::Client,
    sub: &Subscription,
    payload: &str,
) -> Result<StatusCode> {
    deliver_inner(http, sub, payload, false).await
}

pub(super) async fn deliver_live_activity(
    http: &reqwest::Client,
    sub: &Subscription,
    payload: &str,
) -> Result<StatusCode> {
    anyhow::ensure!(sub.app_kind == AppKind::Ios, "Live Activity requires iOS");
    deliver_inner(http, sub, payload, true).await
}

async fn deliver_inner(
    http: &reqwest::Client,
    sub: &Subscription,
    payload: &str,
    live: bool,
) -> Result<StatusCode> {
    validate(sub)?;
    let pem = std::fs::read(std::env::var("POCKYMOE_APNS_KEY_PATH")?)?;
    let token = jwt(
        &pem,
        &std::env::var("POCKYMOE_APNS_KEY_ID")?,
        &std::env::var("POCKYMOE_APNS_TEAM_ID")?,
        chrono::Utc::now().timestamp(),
    )?;
    let event: Value = serde_json::from_str(payload)?;
    let body = if live {
        event.clone()
    } else {
        native_payload(&event, sub)?
    };
    let tag = if live {
        Some("account-activity")
    } else {
        event["tag"].as_str()
    }
    .ok_or_else(|| anyhow::anyhow!("Missing event tag"))?;

    let host = if sub.sandbox {
        "api.sandbox.push.apple.com"
    } else {
        "api.push.apple.com"
    };
    let reply = http
        .post(format!("https://{host}/3/device/{}", sub.device_token))
        .version(reqwest::Version::HTTP_2)
        .bearer_auth(token)
        .header(
            "apns-topic",
            if live {
                format!(
                    "{}.push-type.liveactivity",
                    std::env::var("POCKYMOE_APNS_TOPIC")?
                )
            } else {
                std::env::var(sub.app_kind.topic_env())?
            },
        )
        .header("apns-collapse-id", collapse_id(tag))
        .header(
            "apns-push-type",
            if live { "liveactivity" } else { "alert" },
        )
        .header("apns-priority", "10")
        .header(
            "apns-expiration",
            (chrono::Utc::now().timestamp() + 86400).to_string(),
        )
        .json(&body)
        .send()
        .await?;
    let status = reply.status();
    if status == StatusCode::BAD_REQUEST {
        let reason: Value = reply.json().await.unwrap_or_default();
        if matches!(
            reason["reason"].as_str(),
            Some("BadDeviceToken" | "DeviceTokenNotForTopic")
        ) {
            return Ok(StatusCode::GONE);
        }
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscription_kinds_topics_collapse_and_legacy_json() -> Result<()> {
        let raw = json!({"nativePlatform":"apns","deviceToken":"ab".repeat(32),"relayOrigin":"https://relay.example.com","apnsTopic":"client-controlled"});
        let ios: Subscription = serde_json::from_value(raw.clone())?;
        assert_eq!(ios.app_kind, AppKind::Ios);
        assert!(!ios.sandbox);
        assert_eq!(ios.app_kind.topic_env(), "POCKYMOE_APNS_TOPIC");
        let mut desktop = raw.clone();
        desktop["appKind"] = json!("macos-desktop");
        let desktop: Subscription = serde_json::from_value(desktop)?;
        assert_eq!(desktop.app_kind.topic_env(), "POCKYMOE_APNS_DESKTOP_TOPIC");
        assert_ne!(subscription_id(&ios), subscription_id(&desktop));
        let mut invalid = raw;
        invalid["appKind"] = json!("arbitrary-topic");
        assert!(serde_json::from_value::<Subscription>(invalid).is_err());
        assert_eq!(collapse_id("device:turn"), "device:turn");
        let long = "device:".to_owned() + &"x".repeat(100);
        assert!(collapse_id(&long).len() <= 64);
        assert_eq!(collapse_id(&long), collapse_id(&long));
        assert_ne!(collapse_id(&long), collapse_id(&(long + "x")));
        let payload = native_payload(
            &json!({"title":"Remote Codex","body":"A thread needs your input. Click to respond.","url":"/devices/device/threads/thread","tag":"device:request"}),
            &desktop,
        )?;
        assert_eq!(payload["tag"], "device:request");
        assert_eq!(payload["eventId"], payload["tag"]);
        assert_eq!(payload["url"], "/devices/device/threads/thread");
        assert_eq!(payload["deviceId"], "device");
        assert_eq!(payload["threadId"], "thread");
        assert_eq!(
            payload["aps"]["alert"]["body"],
            "A thread needs your input. Click to respond."
        );
        Ok(())
    }

    #[test]
    fn token_validation_and_signed_provider_jwt() -> Result<()> {
        assert!(validate(&Subscription {
            native_platform: "apns".into(),
            app_kind: AppKind::Ios,
            device_token: "ab".repeat(32),
            relay_origin: "https://relay.example.com".into(),
            sandbox: true
        })
        .is_ok());
        assert!(validate(&Subscription {
            native_platform: "apns".into(),
            app_kind: AppKind::Ios,
            device_token: "../injection".into(),
            relay_origin: "https://relay.example.com".into(),
            sandbox: false
        })
        .is_err());
        let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1)?;
        let key = openssl::ec::EcKey::generate(&group)?;
        let value = jwt(&key.private_key_to_pem()?, "KEY", "TEAM", 123)?;
        let parts: Vec<_> = value.split('.').collect();
        let raw = URL_SAFE_NO_PAD.decode(parts[2])?;
        assert_eq!(raw.len(), 64);
        let sig = EcdsaSig::from_private_components(
            openssl::bn::BigNum::from_slice(&raw[..32])?,
            openssl::bn::BigNum::from_slice(&raw[32..])?,
        )?;
        let hash = openssl::sha::sha256(format!("{}.{}", parts[0], parts[1]).as_bytes());
        assert!(sig.verify(&hash, &key)?);
        Ok(())
    }
}
