//! Shared device-local API connections. Saving never writes harness configuration.
use super::*;
use anyhow::ensure;

const MASK: &str = "[stored privately]";
// Reasoning models may spend output tokens before emitting even a short OK.
// A probe still asks for a tiny response, but must leave room for that work.
pub const CONNECTION_PROBE_MAX_TOKENS: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connection {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "api_key_auth")]
    pub auth_type: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub revision: String,
    // Provider-owned fragments only, retained on migration and never sent in catalog metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub harness_configs: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_harness: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context_windows: BTreeMap<String, i64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub connection_id: String,
    pub profile_id: String,
    pub model: String,
    pub revision: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connections {
    #[serde(default)]
    connections: Vec<Connection>,
    #[serde(default)]
    bindings: BTreeMap<String, Binding>,
    #[serde(default)]
    migrated: Vec<String>,
    #[serde(default)]
    applied: BTreeMap<String, Binding>,
    #[serde(default)]
    backup_bindings: BTreeMap<String, Option<Binding>>,
}
fn read(dir: &Path) -> Result<Connections> {
    match std::fs::read(dir.join("connections.json")) {
        Ok(data) => serde_json::from_slice(&data).context("Unable to read saved connections"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Connections::default()),
        Err(e) => Err(e.into()),
    }
}
fn write(dir: &Path, store: &Connections) -> Result<()> {
    private_write(
        &dir.join("connections.json"),
        &serde_json::to_vec_pretty(store)?,
    )
}
impl Connection {
    pub fn summary(&self) -> Value {
        json!({"id":self.id,"name":self.name,"protocol":self.protocol,"baseUrl":self.base_url,
            "authType":self.auth_type,"model":self.model,"headers":self.headers.keys().map(|k|(k,MASK)).collect::<BTreeMap<_,_>>(),
            "hasApiKey":!self.api_key.is_empty(),"revision":self.revision,"local":true,"sources":[],"legacyHarness":self.legacy_harness})
    }
    pub fn metadata(&self) -> Value {
        json!({"id":self.id,"name":self.name,"protocol":self.protocol,"revision":self.revision})
    }
    pub fn validate(&self) -> Result<()> {
        ensure_protocol(&self.protocol)?;
        if self.name.trim().is_empty()
            || self.name.len() > 120
            || self.name.chars().any(char::is_control)
        {
            bail!("A connection name is required (maximum 120 bytes)");
        }
        if self.model.len() > 200 || self.model.chars().any(char::is_control) {
            bail!("Invalid model identifier");
        }
        if self.api_key.trim().is_empty()
            || self.api_key.len() > 8192
            || self.api_key.chars().any(char::is_control)
        {
            bail!("Provide a valid API key");
        }
        if !["api_key", "bearer"].contains(&self.auth_type.as_str()) {
            bail!("Invalid authentication type");
        }
        let u = url::Url::parse(&self.base_url).map_err(|_| anyhow!("Invalid upstream URL"))?;
        if u.scheme() != "https"
            && !(u.scheme() == "http"
                && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
        {
            bail!("Use HTTPS for upstreams (HTTP is allowed for local services)");
        }
        if u.host_str().is_none()
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
        {
            bail!("Upstream URL must not contain credentials, query parameters or fragments");
        }
        if self.headers.len() > 32 {
            bail!("Too many custom headers");
        }
        for (k, v) in &self.headers {
            let key = k.to_ascii_lowercase();
            if k.len() > 128
                || v.len() > 8192
                || v.chars().any(char::is_control)
                || reqwest::header::HeaderName::from_bytes(k.as_bytes()).is_err()
                || matches!(
                    key.as_str(),
                    "authorization"
                        | "x-api-key"
                        | "x-goog-api-key"
                        | "host"
                        | "content-length"
                        | "connection"
                        | "transfer-encoding"
                        | "cookie"
                        | "proxy-authorization"
                )
                || key.starts_with("x-rcd-")
            {
                bail!("Invalid or reserved custom header");
            }
        }
        if self
            .context_windows
            .values()
            .any(|v| *v < 1 || *v > 100_000_000)
        {
            bail!("Invalid context window");
        }
        for (h, cfg) in &self.harness_configs {
            let mut p = self.profile(h, "validation", "model")?;
            p.settings_config = cfg.clone();
            provider_config::validate_config(&p)?;
        }
        Ok(())
    }
    pub fn compatible(&self, harness: &str) -> bool {
        matches!(
            (self.protocol.as_str(), harness),
            ("openai_responses", "codex" | "grok")
                | ("openai_chat", "grok" | "deepseek")
                | ("anthropic", "claude" | "deepseek")
                | ("gemini", "gemini")
        )
    }
    pub fn validate_application(&self, harness: &str) -> Result<()> {
        ensure!(
            self.compatible(harness),
            "Connection protocol is incompatible with the selected harness"
        );
        ensure!(
            harness != "grok" || self.headers.is_empty(),
            "Grok does not support custom HTTP headers; use this connection with another consumer"
        );
        Ok(())
    }
    pub fn profile(&self, harness: &str, id: &str, model: &str) -> Result<Profile> {
        // Native fragments from another harness must never be projected into this one.
        if !["codex", "claude", "gemini", "grok", "deepseek"].contains(&harness) {
            bail!("Unknown harness");
        }
        let mut cfg = self
            .harness_configs
            .get(harness)
            .cloned()
            .unwrap_or_else(|| json!({}));
        // Remove the migrated header representation before projecting the canonical map.
        if let Some(env) = cfg.get_mut("env").and_then(Value::as_object_mut) {
            env.remove("ANTHROPIC_CUSTOM_HEADERS");
            env.remove("GEMINI_CLI_CUSTOM_HEADERS");
        }
        if let Some(provider) = cfg.get_mut("provider").and_then(Value::as_object_mut) {
            provider.remove("http_headers");
        }
        if let Some(object) = cfg.as_object_mut() {
            object.remove("headers");
        }
        if !self.headers.is_empty() {
            match harness {
                "claude" | "gemini" => {
                    if cfg.get("env").is_none() {
                        cfg["env"] = json!({});
                    }
                    cfg["env"][if harness == "claude" {
                        "ANTHROPIC_CUSTOM_HEADERS"
                    } else {
                        "GEMINI_CLI_CUSTOM_HEADERS"
                    }] = json!(self
                        .headers
                        .iter()
                        .map(|(k, v)| format!("{k}: {v}"))
                        .collect::<Vec<_>>()
                        .join("\n"));
                }
                "deepseek" => cfg["headers"] = json!(self.headers),
                "codex" => {
                    if cfg.get("provider").is_none() {
                        cfg["provider"] = json!({});
                    }
                    cfg["provider"]["http_headers"] = json!(self.headers);
                }
                _ => {}
            }
        }
        Ok(Profile {
            id: id.into(),
            name: self.name.clone(),
            harness: harness.into(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            auth_type: self.auth_type.clone(),
            model: if model.is_empty() {
                self.model.clone()
            } else {
                model.into()
            },
            api_type: match self.protocol.as_str() {
                "openai_chat" => "chat_completions",
                "anthropic" => "anthropic",
                _ => "responses",
            }
            .into(),
            context_window: self
                .context_windows
                .get(harness)
                .copied()
                .unwrap_or_else(context_window),
            settings_config: cfg,
            sort_index: 0,
            revision: None,
            live_revision: None,
        })
    }
    pub fn discovery_profile(&self) -> Result<Profile> {
        self.validate()?;
        self.profile(
            match self.protocol.as_str() {
                "anthropic" => "claude",
                "gemini" => "gemini",
                "openai_chat" => "deepseek",
                _ => "codex",
            },
            "",
            if self.model.is_empty() {
                "discovery"
            } else {
                &self.model
            },
        )
    }
}
fn ensure_protocol(p: &str) -> Result<()> {
    if !["openai_chat", "openai_responses", "anthropic", "gemini"].contains(&p) {
        bail!("Unsupported connection protocol");
    }
    Ok(())
}
fn from_profile(p: &Profile) -> Connection {
    let mut config = p.settings_config.clone();
    provider_config::strip_basic_auth(&mut config);
    Connection {
        id: p.id.clone(),
        name: p.name.clone(),
        protocol: match p.harness.as_str() {
            "claude" => "anthropic",
            "gemini" => "gemini",
            _ => match p.api_type.as_str() {
                "chat_completions" => "openai_chat",
                "anthropic" => "anthropic",
                _ => "openai_responses",
            },
        }
        .into(),
        base_url: p.base_url.clone(),
        api_key: p.api_key.clone(),
        auth_type: p.auth_type.clone(),
        model: p.model.clone(),
        headers: provider_config::headers(p).into_iter().collect(),
        revision: uuid::Uuid::new_v4().to_string(),
        harness_configs: BTreeMap::from([(p.harness.clone(), config)]),
        legacy_harness: Some(p.harness.clone()),
        context_windows: BTreeMap::from([(p.harness.clone(), p.context_window)]),
    }
}
/// Caller serializes mutations with Supervisor.upstream_gate. Never touches native files.
pub fn migrate(dir: &Path) -> Result<()> {
    let mut store = read(dir)?;
    let old = load(dir)?;
    let mut changed = false;
    for p in old.profiles {
        if store.migrated.contains(&p.id) {
            continue;
        }
        let c = from_profile(&p);
        if !store.connections.iter().any(|v| v.id == c.id) {
            if old.active.get(&p.harness) == Some(&p.id) {
                store
                    .bindings
                    .entry(p.harness.clone())
                    .or_insert_with(|| Binding {
                        connection_id: c.id.clone(),
                        profile_id: p.id.clone(),
                        model: p.model.clone(),
                        revision: c.revision.clone(),
                    });
            }
            store.connections.push(c);
        }
        store.migrated.push(p.id);
        changed = true;
    }
    if changed {
        write(dir, &store)?;
    }
    Ok(())
}
pub fn list(dir: &Path) -> Result<Value> {
    let s = read(dir)?;
    Ok(
        json!({"connections":s.connections.iter().map(Connection::summary).collect::<Vec<_>>(),"bindings":effective_bindings(dir,&s)?}),
    )
}
pub fn metadata(dir: &Path) -> Result<Value> {
    Ok(json!(read(dir)?
        .connections
        .iter()
        .map(Connection::metadata)
        .collect::<Vec<_>>()))
}
pub fn get(dir: &Path, id: &str) -> Result<Connection> {
    read(dir)?
        .connections
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("Connection not found"))
}
pub fn prepare(dir: &Path, mut c: Connection) -> Result<Connection> {
    if !c.id.is_empty() {
        uuid::Uuid::parse_str(&c.id).map_err(|_| anyhow!("Invalid connection ID"))?;
        let old = get(dir, &c.id)?;
        if c.revision != old.revision {
            bail!("Connection edit conflict: reload before saving");
        }
        if c.base_url == old.base_url && c.protocol == old.protocol {
            if c.api_key.is_empty() {
                c.api_key = old.api_key;
            }
            for (k, v) in &mut c.headers {
                if v == MASK {
                    *v = old
                        .headers
                        .get(k)
                        .cloned()
                        .ok_or_else(|| anyhow!("Enter the custom header value"))?;
                }
            }
        }
        // API clients cannot overwrite hidden native fragments.
        c.harness_configs = old.harness_configs;
        c.legacy_harness = old.legacy_harness;
        c.context_windows = old.context_windows;
    } else if !c.harness_configs.is_empty()
        || c.legacy_harness.is_some()
        || !c.context_windows.is_empty()
    {
        bail!("Native fragments cannot be supplied when adding a connection");
    }
    if c.headers.values().any(|v| v == MASK) {
        bail!("Enter custom header values for the new endpoint");
    }
    c.validate()?;
    Ok(c)
}
pub fn upsert(dir: &Path, c: Connection) -> Result<Value> {
    let mut c = prepare(dir, c)?;
    let mut s = read(dir)?;
    if c.id.is_empty() {
        c.id = uuid::Uuid::new_v4().to_string();
    }
    c.revision = uuid::Uuid::new_v4().to_string();
    s.connections.retain(|v| v.id != c.id);
    s.connections.push(c.clone());
    write(dir, &s)?;
    Ok(c.summary())
}
pub fn remove(dir: &Path, id: &str) -> Result<()> {
    let mut s = read(dir)?;
    if effective_bindings(dir, &s)?
        .values()
        .any(|v| v.connection_id == id)
    {
        bail!("Connection is in use by a harness");
    }
    s.connections.retain(|c| c.id != id);
    write(dir, &s)
}
pub fn binding(dir: &Path, harness: &str) -> Result<Option<Binding>> {
    Ok(effective_bindings(dir, &read(dir)?)?.remove(harness))
}
fn set_binding(
    dir: &Path,
    harness: &str,
    binding: Binding,
    backup: &str,
    previous: Option<Binding>,
) -> Result<()> {
    let mut s = read(dir)?;
    if !s.migrated.contains(&binding.profile_id) {
        s.migrated.push(binding.profile_id.clone());
    }
    s.applied
        .insert(binding.profile_id.clone(), binding.clone());
    s.bindings.insert(harness.into(), binding);
    s.backup_bindings.insert(backup.into(), previous);
    write(dir, &s)
}
// Native backups retain an active profile ID, which can be reused across revisions.
// Keep the exact prior application alongside each backup instead of inferring it
// from that profile's latest saved contents.
pub(super) fn restore_binding(dir: &Path, backup: &str, harness: &str) -> Result<()> {
    let mut s = read(dir)?;
    let Some(previous) = s.backup_bindings.remove(backup) else {
        return Ok(());
    };
    if let Some(previous) = previous {
        s.applied
            .insert(previous.profile_id.clone(), previous.clone());
        s.bindings.insert(harness.into(), previous);
    } else {
        s.bindings.remove(harness);
    }
    write(dir, &s)
}
pub fn import(dir: &Path, c: Connection, expected: Option<&str>) -> Result<Value> {
    c.validate()?;
    uuid::Uuid::parse_str(&c.id)?;
    uuid::Uuid::parse_str(&c.revision)?;
    let mut s = read(dir)?;
    if let Some(old) = s.connections.iter().find(|v| v.id == c.id) {
        if old.revision == c.revision {
            return Ok(old.summary());
        }
        if expected != Some(old.revision.as_str()) {
            bail!("Connection sync conflict: reload and explicitly update this device");
        }
    } else if expected.is_some() {
        bail!("Connection sync conflict: local copy was removed");
    }
    s.connections.retain(|v| v.id != c.id);
    s.connections.push(c.clone());
    write(dir, &s)?;
    Ok(c.summary())
}

/// Transport for direct text clients such as Content Quality; no harness activation.
pub async fn complete(
    c: &Connection,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: usize,
) -> Result<String> {
    c.validate()?;
    let model = if model.is_empty() {
        c.model.as_str()
    } else {
        model
    };
    if model.is_empty() || model.len() > 200 || model.chars().any(char::is_control) {
        bail!("Configure a valid model");
    }
    if system.contains(&c.api_key) || user.contains(&c.api_key) {
        bail!("Source contains the upstream credential");
    }
    let base = c.base_url.trim_end_matches('/');
    let (endpoint, body) = match c.protocol.as_str() {
        "openai_chat" => (
            format!(
                "{base}{}",
                if base.ends_with("/chat/completions") {
                    ""
                } else {
                    "/chat/completions"
                }
            ),
            json!({"model":model,"stream":false,"temperature":0.2,"max_tokens":max_tokens,"messages":[{"role":"system","content":system},{"role":"user","content":user}]}),
        ),
        "openai_responses" => (
            format!(
                "{base}{}",
                if base.ends_with("/responses") {
                    ""
                } else {
                    "/responses"
                }
            ),
            json!({"model":model,"stream":false,"instructions":system,"input":user,"max_output_tokens":max_tokens}),
        ),
        "anthropic" => (
            format!(
                "{base}/{}messages",
                if base.ends_with("/v1") { "" } else { "v1/" }
            ),
            json!({"model":model,"max_tokens":max_tokens,"system":system,"messages":[{"role":"user","content":user}]}),
        ),
        "gemini" => {
            let mut u = url::Url::parse(&format!(
                "{base}/{v}models/",
                v = if base.ends_with("/v1") || base.ends_with("/v1beta") {
                    ""
                } else {
                    "v1beta/"
                }
            ))?;
            u.path_segments_mut()
                .map_err(|_| anyhow!("Invalid upstream URL"))?
                .pop_if_empty()
                .push(&format!(
                    "{}:generateContent",
                    model.strip_prefix("models/").unwrap_or(model)
                ));
            (
                u.to_string(),
                json!({"systemInstruction":{"parts":[{"text":system}]},"contents":[{"role":"user","parts":[{"text":user}]}],"generationConfig":{"maxOutputTokens":max_tokens,"temperature":0.2}}),
            )
        }
        _ => bail!("Unsupported connection protocol"),
    };
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow!("Cannot initialize upstream client"))?;
    let mut req = client.post(endpoint).json(&body);
    req = match c.protocol.as_str() {
        "gemini" => req.header("x-goog-api-key", &c.api_key),
        "anthropic" => {
            let r = req.header("anthropic-version", "2023-06-01");
            if c.auth_type == "bearer" {
                r.bearer_auth(&c.api_key)
            } else {
                r.header("x-api-key", &c.api_key)
            }
        }
        _ => req.bearer_auth(&c.api_key),
    };
    for (k, v) in &c.headers {
        req = req.header(k, v);
    }
    let mut response = req
        .send()
        .await
        .map_err(|_| anyhow!("Upstream request failed"))?;
    if !response.status().is_success() {
        bail!("Upstream returned HTTP {}", response.status().as_u16());
    }
    let limit = 512 * 1024;
    if response.content_length().unwrap_or(0) > limit as u64 {
        bail!("Upstream response exceeds size limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Cannot read upstream response"))?
    {
        if bytes.len() + chunk.len() > limit {
            bail!("Upstream response exceeds size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    let v: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow!("Invalid upstream response"))?;
    let text = match c.protocol.as_str() {
        "openai_chat" => {
            if v["choices"][0]["finish_reason"]
                .as_str()
                .is_some_and(|s| s != "stop")
            {
                bail!("Upstream response was incomplete");
            }
            v["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .into()
        }
        "openai_responses" => {
            if v["status"] != "completed" {
                bail!("Upstream response was incomplete");
            }
            v["output"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["type"] == "message")
                .flat_map(|row| row["content"].as_array().into_iter().flatten())
                .filter(|row| row["type"] == "output_text")
                .filter_map(|row| row["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        }
        "anthropic" => {
            if v["stop_reason"] != "end_turn" {
                bail!("Upstream response was incomplete");
            }
            v["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["type"] == "text")
                .filter_map(|row| row["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        }
        "gemini" => {
            let candidate = &v["candidates"][0];
            if candidate["finishReason"] != "STOP" {
                bail!("Upstream response was incomplete");
            }
            candidate["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["thought"] != true)
                .filter_map(|row| row["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        }
        _ => unreachable!(),
    };
    if text.trim().is_empty() || text.len() > 128 * 1024 || text.contains(&c.api_key) {
        bail!("Upstream returned invalid or sensitive text");
    }
    Ok(text)
}

/// Legacy editors still save provider-owned configuration, without applying it.
pub fn refresh_legacy(dir: &Path, id: &str) -> Result<()> {
    migrate(dir)?;
    let p = super::profile(dir, id)?;
    let mut s = read(dir)?;
    if let Some(c) = s.connections.iter_mut().find(|c| c.id == id) {
        *c = from_profile(&p);
        write(dir, &s)?;
    }
    Ok(())
}

fn effective_bindings(dir: &Path, s: &Connections) -> Result<BTreeMap<String, Binding>> {
    let old = load(dir)?;
    let mut out = BTreeMap::new();
    for (h, id) in old.active {
        let Some(p) = old.profiles.iter().find(|p| p.id == id) else {
            continue;
        };
        let binding = s
            .bindings
            .get(&h)
            .filter(|b| b.profile_id == id)
            .cloned()
            .or_else(|| s.applied.get(&id).cloned())
            .or_else(|| {
                s.connections.iter().find(|c| c.id == id).map(|c| Binding {
                    connection_id: c.id.clone(),
                    profile_id: id.clone(),
                    model: p.model.clone(),
                    revision: c.revision.clone(),
                })
            });
        if let Some(b) = binding {
            out.insert(h, b);
        }
    }
    Ok(out)
}
pub fn apply(dir: &Path, c: &Connection, harness: &str, model: &str) -> Result<Value> {
    apply_at(dir, c, harness, model, &home(harness))
}
pub fn apply_at(
    dir: &Path,
    c: &Connection,
    harness: &str,
    model: &str,
    root: &Path,
) -> Result<Value> {
    c.validate_application(harness)?;
    let original = load(dir)?;
    let connections_before = read(dir)?;
    let previous_binding = effective_bindings(dir, &connections_before)?.remove(harness);
    // Reuse a consumer profile so native backfill stays with its connection.
    let previous = original.profiles.iter().find(|p| {
        p.harness == harness
            && (connections_before
                .applied
                .get(&p.id)
                .is_some_and(|b| b.connection_id == c.id)
                || p.id == c.id)
    });
    let id = previous
        .map(|p| p.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut effective = c.clone();
    let mut expected = None;
    if let Some(old) = previous {
        let mut config = old.settings_config.clone();
        if original.active.get(harness) == Some(&old.id) {
            let before = live_revision(old, root)?;
            config = provider_config::capture(harness, root)?;
            if before != live_revision(old, root)? {
                bail!(write_engine::CONFLICT);
            }
            expected = Some(before);
        }
        provider_config::strip_basic_auth(&mut config);
        effective.harness_configs.insert(harness.into(), config);
        effective
            .context_windows
            .insert(harness.into(), old.context_window);
    }
    let mut p = effective.profile(harness, &id, model)?;
    p.live_revision = expected;
    if let Some(old) = previous {
        p.sort_index = old.sort_index;
    }
    validate(&p, true)?;
    upsert_at(dir, p.clone(), root)?;
    let backup = match activate_at(dir, &p, root) {
        Ok(b) => b,
        Err(e) => {
            save(dir, &original)?;
            return Err(e);
        }
    };
    let binding = Binding {
        connection_id: c.id.clone(),
        profile_id: id.clone(),
        model: p.model.clone(),
        revision: c.revision.clone(),
    };
    if let Err(e) = set_binding(dir, harness, binding, &backup, previous_binding) {
        super::restore(dir, &backup)?;
        save(dir, &original)?;
        write(dir, &connections_before)?;
        return Err(e);
    }
    Ok(json!({"ok":true,"profileId":id,"backupId":backup}))
}

// Serialized alongside native configuration and profile state by one rollback-
// protected write plan. Historical applied rows remain available for reuse.
pub(super) fn without_binding(dir: &Path, harness: &str) -> Result<String> {
    let mut s = read(dir)?;
    s.bindings.remove(harness);
    Ok(serde_json::to_string_pretty(&s)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(protocol: &str) -> Connection {
        Connection {
            id: String::new(),
            name: "Shared upstream".into(),
            protocol: protocol.into(),
            base_url: "https://example.invalid/v1".into(),
            api_key: "test-fixture-secret-123".into(),
            auth_type: "api_key".into(),
            model: "fixture-model".into(),
            headers: BTreeMap::new(),
            revision: String::new(),
            harness_configs: BTreeMap::new(),
            legacy_harness: None,
            context_windows: BTreeMap::new(),
        }
    }
    fn saved(dir: &Path, protocol: &str) -> Connection {
        let v = upsert(dir, fixture(protocol)).unwrap();
        get(dir, v["id"].as_str().unwrap()).unwrap()
    }
    #[test]
    fn save_is_private_and_does_not_apply_or_touch_native_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("native");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("settings.json"), "{\"unrelated\":true}").unwrap();
        let c = saved(temp.path(), "anthropic");
        assert!(super::super::load(temp.path()).unwrap().active.is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("settings.json")).unwrap(),
            "{\"unrelated\":true}"
        );
        assert!(!list(temp.path()).unwrap().to_string().contains(&c.api_key));
        assert!(!metadata(temp.path())
            .unwrap()
            .to_string()
            .contains("baseUrl"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(temp.path().join("connections.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn edits_preserve_private_values_and_require_revision_and_new_endpoint_key() {
        let temp = tempfile::tempdir().unwrap();
        let mut c = fixture("openai_chat");
        c.headers
            .insert("x-provider-token".into(), "private-header".into());
        let v = upsert(temp.path(), c).unwrap();
        let original = get(temp.path(), v["id"].as_str().unwrap()).unwrap();
        let mut edit = original.clone();
        edit.api_key.clear();
        edit.headers.insert("x-provider-token".into(), MASK.into());
        edit.name = "Renamed".into();
        let v = upsert(temp.path(), edit.clone()).unwrap();
        let updated = get(temp.path(), v["id"].as_str().unwrap()).unwrap();
        assert_eq!(updated.api_key, original.api_key);
        assert_eq!(updated.headers["x-provider-token"], "private-header");
        assert_ne!(updated.revision, original.revision);
        assert!(upsert(temp.path(), edit)
            .unwrap_err()
            .to_string()
            .contains("conflict"));
        let mut edit = updated.clone();
        edit.base_url = "https://other.invalid/v1".into();
        edit.api_key.clear();
        assert!(upsert(temp.path(), edit).is_err());
        let mut edit = updated;
        edit.headers
            .insert("Authorization".into(), "disallowed".into());
        assert!(upsert(temp.path(), edit).is_err());
    }
    #[test]
    fn migration_keeps_identity_native_tuning_and_never_resurrects_deleted_copy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("native");
        let dir = temp.path().join("store");
        let mut p = super::super::tests::fixture("claude");
        p.id = uuid::Uuid::new_v4().to_string();
        p.settings_config = json!({"env":{"ANTHROPIC_DEFAULT_OPUS_MODEL":"tier-opus","ANTHROPIC_CUSTOM_HEADERS":"x-project: original"}});
        p.context_window = 123456;
        super::super::upsert_at(&dir, p.clone(), &root).unwrap();
        migrate(&dir).unwrap();
        let c = get(&dir, &p.id).unwrap();
        let rev = c.revision.clone();
        assert_eq!(c.headers["x-project"], "original");
        assert_eq!(
            c.profile("claude", "", "other-model")
                .unwrap()
                .settings_config["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"],
            "tier-opus"
        );
        assert_eq!(c.profile("claude", "", "m").unwrap().context_window, 123456);
        assert!(!root.exists());
        migrate(&dir).unwrap();
        assert_eq!(get(&dir, &p.id).unwrap().revision, rev);
        remove(&dir, &p.id).unwrap();
        migrate(&dir).unwrap();
        assert!(get(&dir, &p.id).is_err());
    }
    #[test]
    fn sync_is_idempotent_and_conflicts_never_apply_or_overwrite() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let c = saved(a.path(), "gemini");
        import(b.path(), c.clone(), None).unwrap();
        import(b.path(), c.clone(), None).unwrap();
        assert!(binding(b.path(), "gemini").unwrap().is_none());
        assert!(load(b.path()).unwrap().profiles.is_empty());
        let mut changed = c.clone();
        changed.name = "Remote edit".into();
        let v = upsert(a.path(), changed).unwrap();
        let latest = get(a.path(), v["id"].as_str().unwrap()).unwrap();
        assert!(import(b.path(), latest.clone(), None)
            .unwrap_err()
            .to_string()
            .contains("conflict"));
        assert_eq!(get(b.path(), &c.id).unwrap().name, c.name);
        import(b.path(), latest.clone(), Some(&c.revision)).unwrap();
        assert_eq!(get(b.path(), &c.id).unwrap().revision, latest.revision);
        let mut fork = latest;
        fork.name = "Local fork".into();
        upsert(b.path(), fork).unwrap();
        assert!(import(b.path(), c.clone(), Some(&c.revision)).is_err());
    }
    #[test]
    fn explicit_apply_restores_bindings_and_preserves_config_and_failed_apply_rolls_back() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("store");
        let root = temp.path().join("claude");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("settings.json"),
            "{\"permissions\":{\"allow\":[\"Read\"]}} ",
        )
        .unwrap();
        let first = saved(&dir, "anthropic");
        let v = apply_at(&dir, &first, "claude", "chosen-model", &root).unwrap();
        let native: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(native["permissions"]["allow"][0], "Read");
        assert_eq!(native["env"]["ANTHROPIC_MODEL"], "chosen-model");
        assert!(remove(&dir, &first.id).is_err());
        let bound = binding(&dir, "claude").unwrap().unwrap();
        assert_eq!(bound.connection_id, first.id);
        assert_eq!(bound.model, "chosen-model");
        let second = saved(&dir, "anthropic");
        let v2 = apply_at(&dir, &second, "claude", "second-model", &root).unwrap();
        assert_eq!(
            binding(&dir, "claude").unwrap().unwrap().connection_id,
            second.id
        );
        super::super::restore(&dir, v2["backupId"].as_str().unwrap()).unwrap();
        assert_eq!(
            binding(&dir, "claude").unwrap().unwrap().connection_id,
            first.id
        );
        assert_eq!(
            list(&dir).unwrap()["bindings"]["claude"]["connectionId"],
            first.id
        );
        super::super::restore(&dir, v["backupId"].as_str().unwrap()).unwrap();
        assert!(binding(&dir, "claude").unwrap().is_none());
        assert_eq!(list(&dir).unwrap()["bindings"], json!({}));
        std::fs::write(root.join("settings.json"), "invalid json").unwrap();
        let before = std::fs::read(dir.join("profiles.json")).unwrap();
        assert!(apply_at(&dir, &second, "claude", "m", &root).is_err());
        assert_eq!(std::fs::read(dir.join("profiles.json")).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(root.join("settings.json")).unwrap(),
            "invalid json"
        );
    }
    #[test]
    fn switching_reuses_profiles_and_keeps_native_backfill() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("store");
        let root = temp.path().join("claude");
        let first = saved(&dir, "anthropic");
        let second = saved(&dir, "anthropic");
        let v = apply_at(&dir, &first, "claude", "first-model", &root).unwrap();
        let path = root.join("settings.json");
        let mut native: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        native["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"] = json!("manual-tier");
        std::fs::write(&path, native.to_string()).unwrap();
        apply_at(&dir, &second, "claude", "second-model", &root).unwrap();
        let again = apply_at(&dir, &first, "claude", "first-model", &root).unwrap();
        assert_eq!(v["profileId"], again["profileId"]);
        let native: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(native["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "manual-tier");
        assert_eq!(load(&dir).unwrap().profiles.len(), 2);
        let mut changed = first;
        changed.headers.insert("x-project".into(), "new".into());
        let edit = upsert(&dir, changed).unwrap();
        let latest = get(&dir, edit["id"].as_str().unwrap()).unwrap();
        apply_at(&dir, &latest, "claude", "updated-model", &root).unwrap();
        let native: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(native["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "manual-tier");
        assert_eq!(native["env"]["ANTHROPIC_CUSTOM_HEADERS"], "x-project: new");
    }
    #[test]
    fn restoring_reapplied_connection_recovers_the_prior_revision_and_model() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("store");
        let root = temp.path().join("claude");
        let first = saved(&dir, "anthropic");
        let original = apply_at(&dir, &first, "claude", "original-model", &root).unwrap();
        let mut edited = first.clone();
        edited.name = "Edited connection".into();
        upsert(&dir, edited).unwrap();
        let latest = get(&dir, &first.id).unwrap();
        let applied = apply_at(&dir, &latest, "claude", "updated-model", &root).unwrap();
        assert_eq!(original["profileId"], applied["profileId"]);
        super::super::restore(&dir, applied["backupId"].as_str().unwrap()).unwrap();
        let listed = list(&dir).unwrap();
        assert_eq!(listed["bindings"]["claude"]["revision"], first.revision);
        assert_eq!(listed["bindings"]["claude"]["model"], "original-model");
        assert_eq!(listed["connections"][0]["revision"], latest.revision);
        let native: Value =
            serde_json::from_slice(&std::fs::read(root.join("settings.json")).unwrap()).unwrap();
        assert_eq!(native["env"]["ANTHROPIC_MODEL"], "original-model");
    }
    #[tokio::test]
    async fn four_protocols_use_correct_wire_auth_and_extract_only_final_text() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        for (protocol, path, reply) in [
            (
                "openai_chat",
                "/v1/chat/completions",
                json!({"choices":[{"finish_reason":"stop","message":{"content":"Edited text"}}]}),
            ),
            (
                "openai_responses",
                "/v1/responses",
                json!({"status":"completed","output":[{"type":"reasoning","content":[{"type":"output_text","text":"Hidden"}]},{"type":"message","content":[{"type":"output_text","text":"Edited text"}]}]}),
            ),
            (
                "anthropic",
                "/v1/messages",
                json!({"stop_reason":"end_turn","content":[{"type":"thinking","thinking":"Hidden"},{"type":"text","text":"Edited text"}]}),
            ),
            (
                "gemini",
                "/v1/models/fixture-model:generateContent",
                json!({"candidates":[{"finishReason":"STOP","content":{"parts":[{"thought":true,"text":"Hidden"},{"text":"Edited text"}]}}]}),
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let expected_path = path.to_string();
            let protocol = protocol.to_string();
            let server_protocol = protocol.clone();
            let server = tokio::spawn(async move {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let mut buf = [0; 8192];
                loop {
                    let n = sock.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    data.extend_from_slice(&buf[..n]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..end]);
                        let len = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|v| v.parse::<usize>().unwrap())
                            })
                            .unwrap();
                        if data.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8(data).unwrap();
                assert!(text.starts_with(&format!("POST {expected_path} ")));
                let head = text.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
                assert!(head.contains(match server_protocol.as_str() {
                    "anthropic" => "x-api-key: test-fixture-secret-123",
                    "gemini" => "x-goog-api-key: test-fixture-secret-123",
                    _ => "authorization: bearer test-fixture-secret-123",
                }));
                assert!(head.contains("x-project: test-project"));
                let body: Value =
                    serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                assert!(!body.to_string().contains("test-fixture-secret"));
                let bytes = reply.to_string();
                sock.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",bytes.len(),bytes).as_bytes()).await.unwrap();
            });
            let mut c = fixture(&protocol);
            c.base_url = format!("http://{addr}/v1");
            c.headers.insert("x-project".into(), "test-project".into());
            assert_eq!(
                complete(&c, "", "System text", "User text", 100)
                    .await
                    .unwrap(),
                "Edited text",
                "{protocol}"
            );
            server.await.unwrap();
        }
    }
    #[test]
    fn protocol_compatibility_tracks_real_harness_adapters() {
        assert!(!fixture("openai_chat").compatible("codex"));
        assert!(fixture("openai_responses").compatible("codex"));
        assert!(!fixture("openai_responses").compatible("deepseek"));
        assert!(fixture("anthropic").compatible("deepseek"));
        assert!(!fixture("gemini").compatible("claude"));
        let mut headers = fixture("openai_chat");
        headers.headers.insert("x-project".into(), "test".into());
        assert!(headers.validate_application("grok").is_err());
        assert!(headers.validate_application("deepseek").is_ok());
    }
}
