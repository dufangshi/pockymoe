use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use axum::http::{header, HeaderMap};
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OAuthProvider {
    Google,
    Github,
}

impl OAuthProvider {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "google" => Some(Self::Google),
            "github" => Some(Self::Github),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Github => "github",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::Github => "GitHub",
        }
    }
}

#[derive(Clone, Debug)]
struct OAuthCredentials {
    client_id: String,
    client_secret: String,
}

#[derive(Clone, Debug)]
pub struct OAuthConfig {
    pub public_base_url: Option<String>,
    pub web_origin: Option<String>,
    google: Option<OAuthCredentials>,
    github: Option<OAuthCredentials>,
    google_enabled_by_env: bool,
    github_enabled_by_env: bool,
    google_authorize_url: String,
    google_token_url: String,
    google_userinfo_url: String,
    github_authorize_url: String,
    github_token_url: String,
    github_user_url: String,
    github_emails_url: String,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            public_base_url: None,
            web_origin: None,
            google: None,
            github: None,
            google_enabled_by_env: true,
            github_enabled_by_env: true,
            google_authorize_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            google_token_url: "https://oauth2.googleapis.com/token".into(),
            google_userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            github_authorize_url: "https://github.com/login/oauth/authorize".into(),
            github_token_url: "https://github.com/login/oauth/access_token".into(),
            github_user_url: "https://api.github.com/user".into(),
            github_emails_url: "https://api.github.com/user/emails".into(),
        }
    }
}

impl OAuthConfig {
    pub fn from_env() -> Result<Self> {
        let web_origin = nonempty_env("POCKYMOE_WEB_ORIGIN")
            .map(|value| validate_web_origin(&value))
            .transpose()?;
        let google = credentials(
            "POCKYMOE_GOOGLE_OAUTH_CLIENT_ID",
            "POCKYMOE_GOOGLE_OAUTH_CLIENT_SECRET",
        );
        let github = credentials(
            "POCKYMOE_GITHUB_OAUTH_CLIENT_ID",
            "POCKYMOE_GITHUB_OAUTH_CLIENT_SECRET",
        );
        Ok(Self {
            web_origin,
            public_base_url: nonempty_env("POCKYMOE_PUBLIC_BASE_URL")
                .map(|value| value.trim_end_matches('/').to_string()),
            google,
            github,
            google_enabled_by_env: nonempty_env("POCKYMOE_GOOGLE_OAUTH_ENABLED").as_deref()
                != Some("false"),
            github_enabled_by_env: nonempty_env("POCKYMOE_GITHUB_OAUTH_ENABLED").as_deref()
                != Some("false"),
            google_authorize_url: endpoint(
                "POCKYMOE_GOOGLE_OAUTH_AUTHORIZE_URL",
                "https://accounts.google.com/o/oauth2/v2/auth",
            ),
            google_token_url: endpoint(
                "POCKYMOE_GOOGLE_OAUTH_TOKEN_URL",
                "https://oauth2.googleapis.com/token",
            ),
            google_userinfo_url: endpoint(
                "POCKYMOE_GOOGLE_OAUTH_USERINFO_URL",
                "https://openidconnect.googleapis.com/v1/userinfo",
            ),
            github_authorize_url: endpoint(
                "POCKYMOE_GITHUB_OAUTH_AUTHORIZE_URL",
                "https://github.com/login/oauth/authorize",
            ),
            github_token_url: endpoint(
                "POCKYMOE_GITHUB_OAUTH_TOKEN_URL",
                "https://github.com/login/oauth/access_token",
            ),
            github_user_url: endpoint(
                "POCKYMOE_GITHUB_OAUTH_USER_URL",
                "https://api.github.com/user",
            ),
            github_emails_url: endpoint(
                "POCKYMOE_GITHUB_OAUTH_EMAILS_URL",
                "https://api.github.com/user/emails",
            ),
        })
    }

    /// The browser is the passkey trust root; never use the relay callback URL here.
    pub fn browser_origin(&self) -> Option<String> {
        self.web_origin.clone().or_else(|| {
            Url::parse(self.public_base_url.as_deref()?)
                .ok()
                .map(|url| url.origin().ascii_serialization())
        })
    }

    pub fn browser_url(&self, path: &str) -> String {
        self.web_origin
            .as_ref()
            .map(|origin| format!("{origin}{path}"))
            .unwrap_or_else(|| path.to_owned())
    }

    pub fn api_only(&self, headers: &HeaderMap) -> bool {
        self.web_origin.as_ref().is_some_and(|web| {
            !crate::security::origin_allowed(
                &{
                    let mut h = headers.clone();
                    h.insert("origin", web.parse().unwrap());
                    h
                },
                self.public_base_url.as_deref(),
            )
        })
    }

    pub fn available(&self, provider: OAuthProvider) -> bool {
        self.credentials(provider).is_some()
    }

    pub fn initially_enabled(&self, provider: OAuthProvider) -> bool {
        self.available(provider)
            && match provider {
                OAuthProvider::Google => self.google_enabled_by_env,
                OAuthProvider::Github => self.github_enabled_by_env,
            }
    }

    pub fn callback_url(&self, headers: &HeaderMap, provider: OAuthProvider) -> String {
        let base = self.public_base_url.clone().unwrap_or_else(|| {
            let protocol =
                first_header(headers, "x-forwarded-proto").unwrap_or_else(|| "http".into());
            let host = first_header(headers, "x-forwarded-host")
                .or_else(|| header_value(headers, header::HOST.as_str()))
                .unwrap_or_else(|| "localhost:8788".into());
            format!("{protocol}://{host}")
        });
        let base = base
            .replacen("ws://", "http://", 1)
            .replacen("wss://", "https://", 1)
            .trim_end_matches('/')
            .to_string();
        format!("{base}/relay/auth/oauth/{}/callback", provider.as_str())
    }

    pub fn authorization_url(
        &self,
        provider: OAuthProvider,
        callback_url: &str,
        state: &str,
        verifier: &str,
    ) -> Result<String> {
        let credentials = self.credentials(provider).ok_or_else(|| {
            anyhow!(
                "{} OAuth credentials are not configured",
                provider.display_name()
            )
        })?;
        let base = match provider {
            OAuthProvider::Google => &self.google_authorize_url,
            OAuthProvider::Github => &self.github_authorize_url,
        };
        let mut url = Url::parse(base)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("client_id", &credentials.client_id);
            query.append_pair("redirect_uri", callback_url);
            query.append_pair("response_type", "code");
            query.append_pair("state", state);
            query.append_pair("code_challenge", &crate::security::token_hash(verifier));
            query.append_pair("code_challenge_method", "S256");
            query.append_pair(
                "scope",
                match provider {
                    OAuthProvider::Google => "openid email profile",
                    OAuthProvider::Github => "read:user user:email",
                },
            );
            if provider == OAuthProvider::Google {
                query.append_pair("prompt", "select_account");
            }
        }
        Ok(url.to_string())
    }

    pub async fn fetch_identity(
        &self,
        client: &Client,
        provider: OAuthProvider,
        code: &str,
        callback_url: &str,
        verifier: &str,
    ) -> Result<ExternalIdentity> {
        match provider {
            OAuthProvider::Google => {
                self.fetch_google(client, code, callback_url, verifier)
                    .await
            }
            OAuthProvider::Github => {
                self.fetch_github(client, code, callback_url, verifier)
                    .await
            }
        }
    }

    async fn fetch_google(
        &self,
        client: &Client,
        code: &str,
        callback_url: &str,
        verifier: &str,
    ) -> Result<ExternalIdentity> {
        let credentials = self
            .google
            .as_ref()
            .ok_or_else(|| anyhow!("Google OAuth credentials are not configured"))?;
        let body = form_body(&[
            ("code", code),
            ("code_verifier", verifier),
            ("client_id", &credentials.client_id),
            ("client_secret", &credentials.client_secret),
            ("redirect_uri", callback_url),
            ("grant_type", "authorization_code"),
        ]);
        let response = client
            .post(&self.google_token_url)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("Google token exchange failed");
        }
        let token: Value = response.json().await?;
        let access_token = required_string(&token, "access_token", "Google access token")?;
        let response = client
            .get(&self.google_userinfo_url)
            .bearer_auth(access_token)
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("Google profile lookup failed");
        }
        let profile: Value = response.json().await?;
        let subject = required_string(&profile, "sub", "Google subject")?;
        let source_email = required_string(&profile, "email", "Google email")?;
        if profile.get("email_verified").and_then(Value::as_bool) != Some(true) {
            bail!("Google did not provide a verified email address");
        }
        let username = source_email
            .split('@')
            .next()
            .filter(|value| !value.is_empty())
            .or_else(|| profile.get("name").and_then(Value::as_str))
            .unwrap_or("google-user")
            .to_string();
        let email = source_email.to_ascii_lowercase();
        Ok(ExternalIdentity {
            provider: OAuthProvider::Google,
            subject,
            email,
            username,
        })
    }

    async fn fetch_github(
        &self,
        client: &Client,
        code: &str,
        callback_url: &str,
        verifier: &str,
    ) -> Result<ExternalIdentity> {
        let credentials = self
            .github
            .as_ref()
            .ok_or_else(|| anyhow!("GitHub OAuth credentials are not configured"))?;
        let body = form_body(&[
            ("code", code),
            ("code_verifier", verifier),
            ("client_id", &credentials.client_id),
            ("client_secret", &credentials.client_secret),
            ("redirect_uri", callback_url),
        ]);
        let response = client
            .post(&self.github_token_url)
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;
        if !response.status().is_success() {
            bail!("GitHub token exchange failed");
        }
        let token: Value = response.json().await?;
        let access_token = required_string(&token, "access_token", "GitHub access token")?;
        let request = |url: &str| {
            client
                .get(url)
                .header(header::ACCEPT, "application/vnd.github+json")
                .header(header::USER_AGENT, "pockymoe-relay")
                .bearer_auth(access_token.clone())
        };
        let (user, emails) = tokio::join!(
            request(&self.github_user_url).send(),
            request(&self.github_emails_url).send()
        );
        let user = user?;
        let emails = emails?;
        if !user.status().is_success() || !emails.status().is_success() {
            bail!("GitHub profile lookup failed");
        }
        let user: Value = user.json().await?;
        let emails: Value = emails.json().await?;
        let subject = user
            .get("id")
            .and_then(Value::as_u64)
            .map(|value| value.to_string())
            .ok_or_else(|| anyhow!("GitHub did not provide a user id"))?;
        let username = required_string(&user, "login", "GitHub username")?;
        let email = emails
            .as_array()
            .and_then(|values| {
                values
                    .iter()
                    .find(|value| {
                        value.get("primary").and_then(Value::as_bool) == Some(true)
                            && value.get("verified").and_then(Value::as_bool) == Some(true)
                    })
                    .or_else(|| {
                        values.iter().find(|value| {
                            value.get("verified").and_then(Value::as_bool) == Some(true)
                        })
                    })
            })
            .and_then(|value| value.get("email"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| anyhow!("GitHub did not provide a verified email address"))?;
        Ok(ExternalIdentity {
            provider: OAuthProvider::Github,
            subject,
            email,
            username,
        })
    }

    /// Google with test credentials whose token and profile endpoints live at
    /// `provider_base` (a local mock provider).
    #[cfg(test)]
    pub(crate) fn enable_test_google(&mut self, provider_base: &str) {
        self.google = Some(OAuthCredentials {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
        });
        self.google_authorize_url = format!("{provider_base}/authorize");
        self.google_token_url = format!("{provider_base}/token");
        self.google_userinfo_url = format!("{provider_base}/userinfo");
    }

    /// GitHub with test credentials and endpoints under `{provider_base}/github`.
    #[cfg(test)]
    pub(crate) fn enable_test_github(&mut self, provider_base: &str) {
        self.github = Some(OAuthCredentials {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
        });
        self.github_authorize_url = format!("{provider_base}/github/authorize");
        self.github_token_url = format!("{provider_base}/token");
        self.github_user_url = format!("{provider_base}/github/user");
        self.github_emails_url = format!("{provider_base}/github/emails");
    }

    fn credentials(&self, provider: OAuthProvider) -> Option<&OAuthCredentials> {
        match provider {
            OAuthProvider::Google => self.google.as_ref(),
            OAuthProvider::Github => self.github.as_ref(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExternalIdentity {
    pub provider: OAuthProvider,
    pub subject: String,
    pub email: String,
    pub username: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthState {
    provider: String,
    expires_at: u64,
    nonce: String,
}

pub fn sign_state(provider: OAuthProvider, secret: &str) -> Result<String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let mut nonce = [0_u8; 18];
    OsRng.fill_bytes(&mut nonce);
    let payload = OAuthState {
        provider: provider.as_str().to_string(),
        expires_at: now + 10 * 60 * 1000,
        nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
    };
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?);
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
    mac.update(payload.as_bytes());
    let signature =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{payload}.{signature}"))
}

pub fn verify_state(state: &str, provider: OAuthProvider, secret: &str) -> bool {
    let Some((payload, signature)) = state.split_once('.') else {
        return false;
    };
    if payload.is_empty() || signature.is_empty() || signature.contains('.') {
        return false;
    }
    let Ok(actual) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(payload.as_bytes());
    let expected = mac.finalize().into_bytes();
    if actual.len() != expected.len() || !bool::from(actual.ct_eq(expected.as_slice())) {
        return false;
    }
    let Ok(payload) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(payload) = serde_json::from_slice::<OAuthState>(&payload) else {
        return false;
    };
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return false;
    };
    payload.provider == provider.as_str()
        && payload.expires_at > now.as_millis() as u64
        && !payload.nonce.is_empty()
}

fn credentials(client_id: &str, client_secret: &str) -> Option<OAuthCredentials> {
    Some(OAuthCredentials {
        client_id: nonempty_env(client_id)?,
        client_secret: nonempty_env(client_secret)?,
    })
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn endpoint(name: &str, default: &str) -> String {
    nonempty_env(name).unwrap_or_else(|| default.to_string())
}

fn first_header(headers: &HeaderMap, name: &str) -> Option<String> {
    header_value(headers, name)
        .and_then(|value| value.split(',').next().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn form_body(values: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.extend_pairs(values.iter().copied());
    serializer.finish()
}

fn required_string(value: &Value, key: &str, label: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{label} is missing"))
}

/// Accept only a secure origin, with HTTP reserved for local development.
pub(crate) fn validate_web_origin(value: &str) -> Result<String> {
    let url = Url::parse(value).map_err(|_| anyhow!("Invalid POCKYMOE_WEB_ORIGIN"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("POCKYMOE_WEB_ORIGIN must be an HTTPS origin (HTTP is allowed for localhost) without credentials, path, query or fragment");
    }
    Ok(url.origin().ascii_serialization())
}
