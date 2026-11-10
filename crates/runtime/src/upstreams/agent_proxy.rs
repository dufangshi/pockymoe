//! Agents call models through saved connections without holding their keys.
//! The Supervisor forwards a request to the connection's upstream with the saved
//! credential (docs/upstream-connections.md, "Agent access"). Only inference and
//! model-listing routes are reachable, so the key cannot be used for an
//! upstream's files, batches or account administration.
use super::connections::{auth_headers, Connection};
use anyhow::{bail, ensure, Result};
use serde_json::{json, Value};

/// Where one proxied request goes, and the wire format that authenticates it.
#[derive(Debug)]
pub struct Target {
    pub url: url::Url,
    pub protocol: &'static str,
}

const GEMINI_ACTIONS: [&str; 4] = [
    "generateContent",
    "streamGenerateContent",
    "countTokens",
    "embedContent",
];
/// Query parameters that would carry a credential; ours is sent in a header.
const CREDENTIAL_PARAMS: [&str; 4] = ["key", "api_key", "api-key", "access_token"];

/// Maps the path after `/api/agent-models/{connection}/` to an upstream route.
/// A version prefix is optional, so each SDK's usual base URL works:
/// `…/v1` for OpenAI, the bare root for Anthropic and Gemini.
pub fn target(c: &Connection, method: &str, path: &str, query: Option<&str>) -> Result<Target> {
    ensure!(
        path.len() <= 512
            && !path.contains("..")
            && !path.contains("//")
            && !path.contains('\\')
            && path
                .chars()
                .all(|ch| ch.is_ascii_graphic() && !matches!(ch, '?' | '#' | '%')),
        "Unsupported model route"
    );
    let path = path.trim_matches('/');
    let (versioned, tail) = match path.split_once('/') {
        Some((version @ ("v1" | "v1beta"), rest)) => (Some(version), rest),
        _ => (None, path),
    };
    let base = c.base_url.trim_end_matches('/');
    let (protocol, url) = match (method, tail) {
        ("POST", "chat/completions") => ("openai_chat", openai(base, tail)),
        ("POST", "responses") => ("openai_responses", openai(base, tail)),
        ("POST", "embeddings") => (openai_protocol(c), openai(base, tail)),
        ("POST", "messages" | "messages/count_tokens") => ("anthropic", anthropic(base, tail)),
        ("GET", "models") => match (versioned, c.protocol.as_str()) {
            (Some("v1beta"), _) | (_, "gemini") => ("gemini", gemini(base, tail)),
            (_, "anthropic") => ("anthropic", anthropic(base, tail)),
            _ => (openai_protocol(c), openai(base, tail)),
        },
        ("POST", _) => {
            // Gemini: models/{model}:{action}
            let action = tail
                .strip_prefix("models/")
                .and_then(|rest| rest.rsplit_once(':'))
                .filter(|(model, action)| {
                    !model.is_empty() && !model.contains('/') && GEMINI_ACTIONS.contains(action)
                });
            ensure!(action.is_some(), "Unsupported model route");
            ("gemini", gemini(base, tail))
        }
        _ => bail!("Unsupported model route"),
    };
    let mut url = url::Url::parse(&url)?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Unsupported model route"
    );
    let kept: Vec<(String, String)> = url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .filter(|(key, _)| !CREDENTIAL_PARAMS.contains(&key.to_ascii_lowercase().as_str()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    Ok(Target { url, protocol })
}

fn openai_protocol(c: &Connection) -> &'static str {
    if c.protocol == "openai_responses" {
        "openai_responses"
    } else {
        "openai_chat"
    }
}
/// A base URL may already name one route; the others are its siblings.
fn openai(base: &str, tail: &str) -> String {
    let root = base
        .strip_suffix("/chat/completions")
        .or_else(|| base.strip_suffix("/responses"))
        .unwrap_or(base);
    format!("{root}/{tail}")
}
fn anthropic(base: &str, tail: &str) -> String {
    let version = if base.ends_with("/v1") { "" } else { "v1/" };
    format!("{base}/{version}{tail}")
}
fn gemini(base: &str, tail: &str) -> String {
    let version = if base.ends_with("/v1") || base.ends_with("/v1beta") {
        ""
    } else {
        "v1beta/"
    };
    format!("{base}/{version}{tail}")
}

/// Request headers an agent may pass to the upstream. Its own credential
/// headers are never among them.
pub const FORWARDED_REQUEST_HEADERS: [&str; 6] = [
    "content-type",
    "accept",
    "anthropic-version",
    "anthropic-beta",
    "openai-beta",
    "idempotency-key",
];

/// Headers for the upstream request: what the agent may pass, then the saved
/// credential, which always replaces a header of the same name.
pub fn upstream_headers(
    c: &Connection,
    protocol: &str,
    from_agent: &[(String, String)],
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = from_agent
        .iter()
        .filter(|(name, _)| FORWARDED_REQUEST_HEADERS.contains(&name.as_str()))
        .cloned()
        .collect();
    for (name, value) in auth_headers(c, protocol) {
        let name = name.to_ascii_lowercase();
        // The agent's API version wins over the default; nothing else of ours yields.
        if name == "anthropic-version" && headers.iter().any(|(n, _)| n == "anthropic-version") {
            continue;
        }
        headers.retain(|(n, _)| *n != name);
        headers.push((name, value));
    }
    headers
}

/// Response headers worth returning: the body's type and the upstream's own
/// request ids and rate limits. Cookies and anything else stay behind.
pub fn returns_response_header(name: &str) -> bool {
    matches!(
        name,
        "content-type" | "retry-after" | "request-id" | "x-request-id" | "openai-processing-ms"
    ) || name.starts_with("x-ratelimit-")
        || name.starts_with("anthropic-ratelimit-")
}

/// Finds the saved key in a response as it streams by, including across two
/// chunks. An upstream has no reason to echo it; if one does, the agent must
/// not receive it.
pub struct KeyScan {
    key: Vec<u8>,
    tail: Vec<u8>,
}
impl KeyScan {
    pub fn new(c: &Connection) -> Self {
        Self {
            key: c.api_key.as_bytes().to_vec(),
            tail: Vec::new(),
        }
    }
    /// True when the key appears in what has streamed so far.
    pub fn leaks(&mut self, chunk: &[u8]) -> bool {
        if self.key.len() < 8 {
            return false;
        }
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(chunk);
        let found = window.windows(self.key.len()).any(|part| part == self.key);
        let keep = window.len().min(self.key.len() - 1);
        self.tail = window[window.len() - keep..].to_vec();
        found
    }
}

/// What an agent sees of a connection: never its key, headers or upstream URL.
pub fn agent_view(c: &Connection, agent_access: bool, proxy_root: &str) -> Value {
    let base = format!(
        "{}/api/agent-models/{}",
        proxy_root.trim_end_matches('/'),
        c.id
    );
    let protocols = c.protocols();
    let mut sdk = serde_json::Map::new();
    if protocols.iter().any(|p| p.starts_with("openai")) {
        sdk.insert("openai".into(), json!({"baseUrl": format!("{base}/v1")}));
    }
    if protocols.contains(&"anthropic") {
        sdk.insert("anthropic".into(), json!({"baseUrl": base}));
    }
    if protocols.contains(&"gemini") {
        sdk.insert("gemini".into(), json!({"baseUrl": base}));
    }
    let catalog = c.catalog.as_ref();
    json!({
        "id": c.id,
        "name": c.name,
        "protocol": c.protocol,
        "protocols": protocols,
        "defaultModel": c.model,
        "agentAccess": agent_access,
        "sdk": sdk,
        "models": catalog.map(|catalog| catalog.models.iter().map(|m| {
            let mut model = json!({"id": m.id});
            if let Some(window) = m.context_window { model["contextWindow"] = json!(window); }
            if let Some(output) = m.max_output_tokens { model["maxOutputTokens"] = json!(output); }
            if !m.input.is_empty() { model["input"] = json!(m.input); }
            model
        }).collect::<Vec<_>>()),
        "modelsCheckedAt": catalog.map(|catalog| catalog.checked_at.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(protocol: &str, base_url: &str) -> Connection {
        serde_json::from_value(json!({
            "id": "6f0f3c2e-58c6-4a8b-9d53-0c1f0b1f2a11", "name": "n", "protocol": protocol,
            "baseUrl": base_url, "apiKey": "sk-secret-key-123456", "model": "m",
        }))
        .unwrap()
    }
    fn url(c: &Connection, method: &str, path: &str, query: Option<&str>) -> String {
        let target = target(c, method, path, query).unwrap();
        format!("{} {}", target.protocol, target.url)
    }

    #[test]
    fn each_sdk_base_url_reaches_its_route() {
        let openai = connection("openai_chat", "https://up.example/v1");
        assert_eq!(
            url(&openai, "POST", "v1/chat/completions", None),
            "openai_chat https://up.example/v1/chat/completions"
        );
        assert_eq!(
            url(&openai, "POST", "chat/completions", None),
            "openai_chat https://up.example/v1/chat/completions"
        );
        assert_eq!(
            url(&openai, "POST", "v1/responses", None),
            "openai_responses https://up.example/v1/responses"
        );
        assert_eq!(
            url(&openai, "POST", "v1/embeddings", None),
            "openai_chat https://up.example/v1/embeddings"
        );
        assert_eq!(
            url(&openai, "GET", "v1/models", None),
            "openai_chat https://up.example/v1/models"
        );
        // A multi-format endpoint serves the other formats from the same base.
        assert_eq!(
            url(&openai, "POST", "v1/messages", None),
            "anthropic https://up.example/v1/messages"
        );
        let full = connection("openai_responses", "https://up.example/api/v1/responses/");
        assert_eq!(
            url(&full, "POST", "v1/chat/completions", None),
            "openai_chat https://up.example/api/v1/chat/completions"
        );
        assert_eq!(
            url(&full, "POST", "v1/embeddings", None),
            "openai_responses https://up.example/api/v1/embeddings"
        );

        let anthropic = connection("anthropic", "https://claude.example");
        assert_eq!(
            url(&anthropic, "POST", "v1/messages", None),
            "anthropic https://claude.example/v1/messages"
        );
        assert_eq!(
            url(&anthropic, "POST", "v1/messages/count_tokens", None),
            "anthropic https://claude.example/v1/messages/count_tokens"
        );
        assert_eq!(
            url(&anthropic, "GET", "v1/models", Some("limit=5")),
            "anthropic https://claude.example/v1/models?limit=5"
        );

        let gemini = connection("gemini", "https://g.example");
        assert_eq!(
            url(
                &gemini,
                "POST",
                "v1beta/models/gemini-pro:streamGenerateContent",
                Some("alt=sse&key=agent-token")
            ),
            "gemini https://g.example/v1beta/models/gemini-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            url(&gemini, "GET", "v1beta/models", None),
            "gemini https://g.example/v1beta/models"
        );
    }

    #[test]
    fn only_inference_and_model_listing_are_reachable() {
        let c = connection("openai_chat", "https://up.example/v1");
        for (method, path) in [
            ("POST", "v1/files"),
            ("GET", "v1/files"),
            ("DELETE", "v1/models/gpt"),
            ("POST", "v1/fine_tuning/jobs"),
            ("GET", "v1/organization/api_keys"),
            ("GET", "v1/chat/completions"),
            ("POST", "v1/../admin"),
            ("POST", "v1/chat/completions/../../files"),
            ("POST", "v1//chat/completions"),
            ("POST", "v1/models/x:deleteModel"),
            ("POST", "v1/models/a/b:generateContent"),
            ("POST", "v1/chat/completions%2F..%2Ffiles"),
            ("POST", ""),
        ] {
            assert!(target(&c, method, path, None).is_err(), "{method} {path}");
        }
    }

    #[test]
    fn the_saved_credential_replaces_whatever_the_agent_sent() {
        let c = connection("anthropic", "https://claude.example");
        let sent = vec![
            (
                "authorization".to_string(),
                "Bearer thread-token".to_string(),
            ),
            ("x-api-key".to_string(), "thread-token".to_string()),
            ("anthropic-version".to_string(), "2024-10-22".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("cookie".to_string(), "a=b".to_string()),
        ];
        let headers = upstream_headers(&c, "anthropic", &sent);
        let value = |name: &str| {
            headers
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(value("x-api-key"), ["sk-secret-key-123456"]);
        assert_eq!(value("anthropic-version"), ["2024-10-22"]);
        assert_eq!(value("content-type"), ["application/json"]);
        assert!(value("authorization").is_empty() && value("cookie").is_empty());
        let openai = connection("openai_chat", "https://up.example/v1");
        assert_eq!(
            upstream_headers(&openai, "openai_chat", &sent)
                .iter()
                .find(|(n, _)| n == "authorization")
                .unwrap()
                .1,
            "Bearer sk-secret-key-123456"
        );
    }

    #[test]
    fn the_key_is_found_across_chunks_and_the_view_never_has_it() {
        let c = connection("openai_chat", "https://up.example/v1");
        let mut scan = KeyScan::new(&c);
        assert!(!scan.leaks(b"data: hello sk-secret-"));
        assert!(scan.leaks(b"key-123456 tail"));
        let mut clean = KeyScan::new(&c);
        assert!(!clean.leaks(b"data: one") && !clean.leaks(b"data: two"));
        let view = agent_view(&c, true, "http://127.0.0.1:8787/").to_string();
        assert!(!view.contains("sk-secret") && !view.contains("up.example"));
        assert!(view.contains(
            "http://127.0.0.1:8787/api/agent-models/6f0f3c2e-58c6-4a8b-9d53-0c1f0b1f2a11/v1"
        ));
    }
}
