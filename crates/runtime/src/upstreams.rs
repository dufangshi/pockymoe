// Provider live backfill / switching adapted from CC Switch services/provider/live.rs.
// Copyright (c) 2025 Jason Young. MIT; see THIRD_PARTY_NOTICES.md.
//! Device-local upstream profiles. Only provider-owned fields are changed; no
//! imported commands, plugins, or arbitrary filesystem paths are executed.
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use toml_edit::{value, DocumentMut};
mod cc_switch;
pub mod connections;
mod discovery;
mod dsh;
mod model_meta;
mod provider_config;
mod write_engine;
pub use discovery::{discover_models, discovery_profile, DiscoveryInput};
pub(crate) use discovery::{normalize_effort, DiscoveredModel};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub harness: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "api_key_auth")]
    pub auth_type: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "responses")]
    pub api_type: String,
    #[serde(default = "context_window")]
    pub context_window: i64,
    #[serde(default = "empty_config")]
    pub settings_config: Value,
    #[serde(default)]
    pub sort_index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_revision: Option<String>,
}
fn empty_config() -> Value {
    json!({})
}
fn responses() -> String {
    "responses".into()
}
fn context_window() -> i64 {
    500_000
}
fn api_key_auth() -> String {
    "api_key".into()
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Store {
    profiles: Vec<Profile>,
    active: BTreeMap<String, String>,
    backups: Vec<Backup>,
    /// Original native model entries, retained until managed configuration is removed.
    #[serde(default)]
    grok_models: BTreeMap<String, Option<String>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Backup {
    id: String,
    harness: String,
    created_at: String,
    active: Option<String>,
    files: Vec<FileSnapshot>,
}
#[derive(Clone, Serialize, Deserialize)]
struct FileSnapshot {
    path: PathBuf,
    content: Option<String>,
}

pub fn directory(database: &Path) -> PathBuf {
    database.with_extension("upstreams")
}
fn load(dir: &Path) -> Result<Store> {
    match std::fs::read(dir.join("profiles.json")) {
        Ok(data) => serde_json::from_slice(&data).context("Unable to read saved upstream profiles"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(e.into()),
    }
}
pub fn private_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Invalid configuration path"))?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    tmp.write_all(data)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn save(dir: &Path, store: &Store) -> Result<()> {
    private_write(
        &dir.join("profiles.json"),
        &serde_json::to_vec_pretty(store)?,
    )
}
fn redacted(p: &Profile) -> Value {
    let mut v = serde_json::to_value(p).unwrap();
    v.as_object_mut().unwrap().remove("apiKey");
    v["hasApiKey"] = json!(!p.api_key.is_empty());
    v["settingsConfig"] = provider_config::redact(&p.settings_config);
    v["revision"] = json!(provider_config::revision(p));
    v
}
pub fn inventory(dir: &Path) -> Result<Value> {
    let mut s = load(dir)?;
    s.profiles.sort_by_key(|p| p.sort_index);
    Ok(
        json!({"profiles":s.profiles.iter().map(redacted).collect::<Vec<_>>(),"active":s.active,
        "backups":s.backups.iter().map(|b| json!({"id":b.id,"harness":b.harness,"createdAt":b.created_at})).collect::<Vec<_>>() }),
    )
}
pub fn validate(p: &Profile, require_key: bool) -> Result<()> {
    if !["api_key", "bearer"].contains(&p.auth_type.as_str()) {
        bail!("Invalid authentication type");
    }
    if !["codex", "claude", "gemini", "grok", "deepseek"].contains(&p.harness.as_str()) {
        bail!("Upstream configuration supports OpenAI Codex, Claude Agent, Gemini CLI, Grok Build and DeepSeek Harness");
    }
    provider_config::validate_config(p)?;
    if p.name.trim().is_empty()
        || p.name.len() > 120
        || p.model.trim().is_empty()
        || p.model.len() > 200
    {
        bail!("A name and model are required (maximum 120/200 characters)");
    }
    if p.api_key.len() > 8192
        || p.api_key.contains(['\r', '\n'])
        || (require_key && p.api_key.trim().is_empty())
    {
        bail!("Provide a valid API key");
    }
    let url = url::Url::parse(&p.base_url).map_err(|_| anyhow!("Invalid upstream URL"))?;
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    {
        bail!("Use HTTPS for upstreams (HTTP is allowed for local services)");
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Upstream URL must not contain credentials, query parameters or fragments");
    }
    if !["responses", "chat_completions", "anthropic"].contains(&p.api_type.as_str())
        || p.context_window < 1
        || p.context_window > 100_000_000
    {
        bail!("Invalid API type or context window");
    }
    if p.harness == "codex" && p.api_type != "responses" {
        bail!("Codex requires a Responses API upstream");
    }
    Ok(())
}
pub fn upsert(dir: &Path, p: Profile) -> Result<Value> {
    let root = home(&p.harness);
    upsert_at(dir, p, &root)
}
fn upsert_at(dir: &Path, mut p: Profile, root: &Path) -> Result<Value> {
    let mut s = load(dir)?;
    if let Some(expected) = &p.live_revision {
        if expected != &live_revision(&p, root)? {
            bail!(write_engine::CONFLICT);
        }
    }
    if p.id.is_empty() {
        p.sort_index = s
            .profiles
            .iter()
            .filter(|old| old.harness == p.harness)
            .map(|p| p.sort_index)
            .max()
            .unwrap_or(-1)
            + 1;
        p.id = uuid::Uuid::new_v4().to_string();
    } else if uuid::Uuid::parse_str(&p.id).is_err() {
        bail!("Invalid profile ID");
    }
    if let Some(old) = s.profiles.iter().find(|old| old.id == p.id) {
        if old.harness != p.harness && s.active.get(&old.harness) == Some(&old.id) {
            bail!("Cannot change the harness of an active provider");
        }
        if p.revision
            .as_ref()
            .is_some_and(|revision| revision != &provider_config::revision(old))
        {
            bail!("Provider edit conflict: configuration changed. Reload before saving.");
        }
        let private_source =
            if s.active.get(&old.harness) == Some(&old.id) && p.live_revision.is_some() {
                let live = read_live_profile(&old.harness, &old.name, root)?;
                (live.base_url == p.base_url && live.harness == p.harness)
                    .then_some((live.settings_config, live.api_key))
            } else {
                (old.base_url == p.base_url && old.harness == p.harness)
                    .then_some((old.settings_config.clone(), old.api_key.clone()))
            };
        if let Some((config, key)) = private_source {
            provider_config::restore_redacted(&mut p.settings_config, &config);
            if p.api_key.is_empty() {
                p.api_key = key;
            }
        } else {
            provider_config::remove_redacted(&mut p.settings_config);
        }
    } else {
        provider_config::remove_redacted(&mut p.settings_config);
    }
    validate(&p, true)?;
    p.revision = None;
    p.live_revision = None;
    s.profiles.retain(|old| old.id != p.id);
    s.profiles.push(p.clone());
    save(dir, &s)?;
    Ok(redacted(&p))
}
pub fn remove(dir: &Path, id: &str) -> Result<()> {
    let mut s = load(dir)?;
    if let Some(harness) = s
        .active
        .iter()
        .find(|(_, v)| *v == id)
        .map(|(h, _)| h.clone())
    {
        let baseline = s.backups.iter().find(|b| b.harness == harness && b.active.is_none())
            .cloned().ok_or_else(|| anyhow!("No original configuration backup is available. Switch to another upstream before deleting this one."))?;
        let mut rollback = baseline.clone();
        for file in &mut rollback.files {
            file.content = read(&file.path)?;
        }
        if let Err(error) = restore_files(&baseline) {
            restore_files(&rollback)?;
            return Err(error);
        }
        s.active.remove(&harness);
        s.backups.retain(|b| b.harness != harness);
        if harness == "grok" {
            s.grok_models.clear();
        }
        s.profiles.retain(|p| p.id != id);
        if let Err(error) = save(dir, &s) {
            restore_files(&rollback)?;
            return Err(error);
        }
        return Ok(());
    }
    s.profiles.retain(|p| p.id != id);
    save(dir, &s)
}
pub fn active_profile(dir: &Path, harness: &str) -> Result<Option<Profile>> {
    let s = load(dir)?;
    Ok(s.active
        .get(harness)
        .and_then(|id| s.profiles.iter().find(|p| &p.id == id))
        .cloned())
}

pub(crate) fn prepare_grok_models(
    dir: &Path,
    p: &Profile,
    models: &[DiscoveredModel],
) -> Result<()> {
    prepare_grok_models_at(dir, p, models, &home("grok"))
}
fn prepare_grok_models_at(
    dir: &Path,
    p: &Profile,
    models: &[DiscoveredModel],
    root: &Path,
) -> Result<()> {
    let path = root.join("config.toml");
    let mut doc = toml_doc(&path)?;
    let mut store = load(dir)?;
    configure_grok_models(
        &mut doc,
        p,
        models.iter().map(|m| m.id.as_str()),
        &mut store.grok_models,
    )?;
    for model in models {
        let Some(cap) = &model.reasoning else {
            continue;
        };
        for id in [&model.id, &format!("remote-codex/{}", model.id)] {
            let table = doc["model"][id].as_table_like_mut().unwrap();
            table.remove("reasoning_effort");
            table.remove("supports_reasoning_effort");
            let mut options = toml_edit::Array::new();
            for effort in &cap.efforts {
                let mut entry = toml_edit::InlineTable::new();
                entry.insert("value", effort.reasoning_effort.clone().into());
                entry.insert("label", effort.description.clone().into());
                if cap.default.as_deref() == Some(&effort.reasoning_effort) {
                    entry.insert("default", true.into());
                }
                options.push(entry);
            }
            table.insert("reasoning_efforts", value(options));
            if cap.efforts.is_empty() {
                table.insert("supports_reasoning_effort", value(false));
            }
        }
    }
    // Retain the union of original entries before writing, including across
    // switches. A crash between these atomic writes cannot lose an original.
    save(dir, &store)?;
    private_write(&path, doc.to_string().as_bytes())
}

fn configure_grok_models<'a>(
    doc: &mut DocumentMut,
    p: &'a Profile,
    models: impl Iterator<Item = &'a str>,
    originals: &mut BTreeMap<String, Option<String>>,
) -> Result<()> {
    for section in ["model", "models"] {
        if doc.get(section).is_some_and(|v| !v.is_table_like()) {
            bail!("Invalid Grok {section} configuration section");
        }
    }
    for (id, original) in originals.iter() {
        if let Some(original) = original {
            let snapshot = original.parse::<DocumentMut>()?;
            let entry = snapshot
                .get("model")
                .and_then(|m| m.get(id))
                .ok_or_else(|| anyhow!("Invalid saved Grok model configuration"))?;
            doc["model"][id] = entry.clone();
        } else if let Some(table) = doc.get_mut("model").and_then(|v| v.as_table_like_mut()) {
            table.remove(id);
        }
    }
    // Legacy aliases remain loadable, but selection uses the actual ID. Grok
    // canonicalizes aliases to their wire model and otherwise falls back to
    // built-in authentication when that ID also exists in its built-in catalog.
    if let Some(table) = doc.get_mut("model").and_then(|v| v.as_table_like_mut()) {
        let owned = table
            .iter()
            .filter(|(key, _)| key.starts_with("remote-codex/"))
            .map(|(key, _)| key.to_owned())
            .collect::<Vec<_>>();
        for key in owned {
            table.remove(&key);
        }
    }
    for model in std::iter::once(p.model.as_str()).chain(models) {
        if doc
            .get("model")
            .and_then(|m| m.get(model))
            .is_some_and(|v| !v.is_table_like())
        {
            bail!("Invalid Grok model configuration entry");
        }
        originals.entry(model.to_owned()).or_insert_with(|| {
            doc.get("model").and_then(|m| m.get(model)).map(|entry| {
                let mut snapshot = DocumentMut::new();
                snapshot["model"] = toml_edit::Item::Table(toml_edit::Table::new());
                snapshot["model"][model] = entry.clone();
                snapshot.to_string()
            })
        });
        let alias = format!("remote-codex/{model}");
        for id in [model, alias.as_str()] {
            for (key, text) in [
                ("name", model),
                ("model", model),
                ("base_url", p.base_url.as_str()),
                ("api_key", p.api_key.as_str()),
                ("api_backend", p.api_type.as_str()),
            ] {
                doc["model"][id][key] = value(text);
            }
            // Explicit inline credentials must not compete with an existing helper,
            // provider, environment key, or custom Authorization header.
            for key in [
                "auth_provider",
                "env_key",
                "model_provider",
                "api_base_url",
                "extra_headers",
                "env_http_headers",
                "query_params",
                "mtls_cert_dir",
            ] {
                doc["model"][id].as_table_like_mut().unwrap().remove(key);
            }
            doc["model"][id]["context_window"] = value(p.context_window);
        }
    }
    if doc
        .get("model")
        .and_then(|m| m.get("remote-codex"))
        .is_some()
    {
        doc["model"]["remote-codex"]["name"] = value(&p.model);
    }
    doc["models"]["default"] = value(&p.model);
    Ok(())
}
pub fn profile(dir: &Path, id: &str) -> Result<Profile> {
    load(dir)?
        .profiles
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("Profile not found"))
}
fn home(harness: &str) -> PathBuf {
    let (key, suffix) = match harness {
        "codex" => ("CODEX_HOME", ".codex"),
        "claude" => ("CLAUDE_CONFIG_DIR", ".claude"),
        "grok" => ("GROK_HOME", ".grok"),
        "deepseek" => ("DSH_HOME", ".dsh"),
        _ => ("GEMINI_CLI_HOME", ".gemini"),
    };
    let explicit = std::env::var_os(key)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    if harness == "gemini" {
        explicit
            .unwrap_or_else(crate::config::home_dir)
            .join(".gemini")
    } else {
        explicit.unwrap_or_else(|| crate::config::home_dir().join(suffix))
    }
}
fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
// Re-read native credentials for every new ACP process. An inherited shell key
// must not silently override the upstream selected in device settings.
pub(crate) fn launch_environment(harness: &str) -> Vec<(&'static str, String)> {
    let root = home(harness);
    let mut env = vec![];
    if harness == "claude" {
        if let Ok(doc) = json_doc(&root.join("settings.json")) {
            for key in [
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_MODEL",
            ] {
                if let Some(v) = doc
                    .get("env")
                    .and_then(|e| e.get(key))
                    .and_then(Value::as_str)
                {
                    env.push((key, v.into()));
                }
            }
            if env.iter().any(|(key, _)| *key == "ANTHROPIC_AUTH_TOKEN") {
                env.push(("ANTHROPIC_API_KEY", String::new()));
            } else if env.iter().any(|(key, _)| *key == "ANTHROPIC_API_KEY") {
                env.push(("ANTHROPIC_AUTH_TOKEN", String::new()));
            }
        }
    } else if harness == "gemini" {
        if let Ok(Some(content)) = read(&root.join(".env")) {
            for key in ["GEMINI_API_KEY", "GOOGLE_GEMINI_BASE_URL", "GEMINI_MODEL"] {
                if let Some(v) = content
                    .lines()
                    .rev()
                    .find_map(|l| l.strip_prefix(&format!("{key}=")))
                {
                    // Managed values are JSON-quoted to preserve punctuation.
                    if let Ok(v) = serde_json::from_str::<String>(v) {
                        env.push((key, v));
                    }
                }
            }
        }
    }
    env
}
fn toml_doc(path: &Path) -> Result<DocumentMut> {
    let doc: DocumentMut = read(path)?
        .unwrap_or_default()
        .parse()
        .map_err(|_| anyhow!("Existing TOML is invalid; repair it before switching upstreams"))?;
    for key in ["model_providers", "models", "model"] {
        if key == "model" && doc.get(key).is_some_and(|v| v.is_str()) {
            continue;
        }
        if let Some(section) = doc.get(key) {
            if !section.is_table_like() {
                bail!("Invalid {key} configuration section");
            }
            for child in ["remote_codex", "remote-codex"] {
                if section.get(child).is_some_and(|v| !v.is_table_like()) {
                    bail!("Invalid managed provider section");
                }
            }
        }
    }
    Ok(doc)
}
fn json_doc(path: &Path) -> Result<Value> {
    let v = serde_json::from_str::<Value>(&read(path)?.unwrap_or_else(|| "{}".into()))
        .map_err(|_| anyhow!("Existing JSON is invalid; repair it before switching upstreams"))?;
    if !v.is_object() {
        bail!("Existing configuration must be an object");
    }
    Ok(v)
}
fn put_json(v: &mut Value, key: &str, field: &str, content: Value) -> Result<()> {
    if v.get(key).is_none() {
        v[key] = json!({});
    }
    if !v[key].is_object() {
        bail!("Existing configuration section {key} must be an object");
    }
    v[key][field] = content;
    Ok(())
}
fn changes(p: &Profile, root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let mut output = vec![];
    match p.harness.as_str() {
        "codex" => {
            let path = root.join("config.toml");
            let mut doc = toml_doc(&path)?;
            provider_config::project_codex(&mut doc, &p.settings_config)?;
            doc["model_provider"] = value("remote_codex");
            doc["model"] = value(&p.model);
            doc["model_providers"]["remote_codex"]["name"] = value(&p.name);
            doc["model_providers"]["remote_codex"]["base_url"] = value(&p.base_url);
            doc["model_providers"]["remote_codex"]["wire_api"] = value("responses");
            doc["model_providers"]["remote_codex"]["requires_openai_auth"] = value(true);
            output.push((path, doc.to_string()));
            let path = root.join("auth.json");
            let mut auth = json_doc(&path)?;
            auth["OPENAI_API_KEY"] = json!(p.api_key);
            auth["auth_mode"] = json!("apikey");
            auth.as_object_mut().unwrap().remove("tokens");
            output.push((path, serde_json::to_string_pretty(&auth)?));
        }
        "claude" => {
            let path = root.join("settings.json");
            let mut doc = json_doc(&path)?;
            provider_config::project_claude(&mut doc, &p.settings_config)?;
            let (auth, other) = if p.auth_type == "bearer" {
                ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")
            } else {
                ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN")
            };
            for (key, v) in [
                ("ANTHROPIC_BASE_URL", &p.base_url),
                (auth, &p.api_key),
                ("ANTHROPIC_MODEL", &p.model),
            ] {
                put_json(&mut doc, "env", key, json!(v))?;
            }
            doc["env"].as_object_mut().unwrap().remove(other);
            output.push((path, serde_json::to_string_pretty(&doc)?));
        }
        "gemini" => {
            let path = root.join("settings.json");
            let mut doc = json_doc(&path)?;
            if doc.get("security").is_none() {
                doc["security"] = json!({});
            }
            if !doc["security"].is_object() {
                bail!("Invalid Gemini security settings");
            }
            put_json(
                &mut doc["security"],
                "auth",
                "selectedType",
                json!("gemini-api-key"),
            )?;
            put_json(&mut doc, "model", "name", json!(p.model))?;
            output.push((path, serde_json::to_string_pretty(&doc)?));
            let path = root.join(".env");
            let old = read(&path)?.unwrap_or_default();
            let old = provider_config::project_gemini_env(&old, &p.settings_config)?;
            let mut lines: Vec<String> = old
                .lines()
                .filter(|line| {
                    !["GEMINI_API_KEY", "GOOGLE_GEMINI_BASE_URL", "GEMINI_MODEL"]
                        .iter()
                        .any(|key| {
                            line.trim_start()
                                .strip_prefix(key)
                                .is_some_and(|s| s.trim_start().starts_with('='))
                        })
                })
                .map(str::to_owned)
                .collect();
            for (k, v) in [
                ("GEMINI_API_KEY", &p.api_key),
                ("GOOGLE_GEMINI_BASE_URL", &p.base_url),
                ("GEMINI_MODEL", &p.model),
            ] {
                lines.push(format!("{k}={}", serde_json::to_string(v)?));
            }
            output.push((path, format!("{}\n", lines.join("\n"))));
        }
        "grok" => {
            let path = root.join("config.toml");
            let mut doc = toml_doc(&path)?;
            configure_grok_models(&mut doc, p, std::iter::empty(), &mut BTreeMap::new())?;
            output.push((path, doc.to_string()));
        }
        "deepseek" => return dsh::changes(p, root),
        _ => bail!("Unsupported harness"),
    }
    Ok(output)
}
pub fn activate(dir: &Path, id: &str) -> Result<String> {
    let p = profile(dir, id)?;
    activate_at(dir, &p, &home(&p.harness))
}
fn native_paths(p: &Profile, root: &Path) -> Result<Vec<PathBuf>> {
    Ok(match p.harness.as_str() {
        "codex" => vec![root.join("config.toml"), root.join("auth.json")],
        "claude" => vec![root.join("settings.json")],
        "gemini" => vec![root.join("settings.json"), root.join(".env")],
        "grok" => vec![root.join("config.toml")],
        "deepseek" => return dsh::paths(p, root),
        _ => bail!("Unsupported harness"),
    })
}
fn activate_at(dir: &Path, target: &Profile, root: &Path) -> Result<String> {
    validate(target, true)?;
    let mut s = load(dir)?;
    let files = native_paths(target, root)?
        .into_iter()
        .map(|path| {
            Ok(FileSnapshot {
                content: read(&path)?,
                path,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if let Some(expected) = &target.live_revision {
        use sha2::{Digest, Sha256};
        let current = hex::encode(Sha256::digest(serde_json::to_vec(
            &files
                .iter()
                .map(|file| file.content.clone())
                .collect::<Vec<_>>(),
        )?));
        if expected != &current {
            bail!(write_engine::CONFLICT);
        }
    }
    let mut p = target.clone();
    let previous = s.active.get(&p.harness).cloned();
    if let Some(id) = &previous {
        if id != &p.id {
            if let Some(old) = s.profiles.iter_mut().find(|old| &old.id == id) {
                // Backfill before stripping: manually adjusted tier mappings,
                // headers and exclusive knobs travel with the outgoing provider.
                old.settings_config = provider_config::capture(&p.harness, root)?;
                if let Ok(live) = read_live_profile(&p.harness, &old.name, root) {
                    old.base_url = live.base_url;
                    old.api_key = live.api_key;
                    old.model = live.model;
                    old.auth_type = live.auth_type;
                    old.api_type = live.api_type;
                    old.context_window = live.context_window;
                }
            }
        }
    } else if ["claude", "codex", "gemini"].contains(&p.harness.as_str()) {
        // Adopt unowned tuning on the first managed activation, including
        // legacy profiles and explicit presets. Supplied fields take precedence.
        let mut native = provider_config::capture(&p.harness, root)?;
        provider_config::strip_basic_auth(&mut native);
        provider_config::merge_defaults(&mut p.settings_config, native);
        if let Some(saved) = s.profiles.iter_mut().find(|old| old.id == p.id) {
            saved.settings_config = p.settings_config.clone();
        }
    }
    let edits = if p.harness == "grok" {
        let path = root.join("config.toml");
        let mut doc = toml_doc(&path)?;
        configure_grok_models(&mut doc, &p, std::iter::empty(), &mut s.grok_models)?;
        vec![(path, doc.to_string())]
    } else {
        changes(&p, root)?
    };
    // Plan was computed from the same observed generation as the snapshot.
    for file in &files {
        if read(&file.path)? != file.content {
            bail!(write_engine::CONFLICT);
        }
    }
    let staged = edits
        .iter()
        .map(|(path, text)| write_engine::stage(path, text.as_bytes()))
        .collect::<Result<Vec<_>>>()?;
    let backup = Backup {
        id: uuid::Uuid::new_v4().to_string(),
        harness: p.harness.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        active: previous,
        files,
    };
    s.backups.push(backup.clone());
    save(dir, &s)?;
    let mut committed = Vec::new();
    let result = (|| -> Result<()> {
        for ((path, text), staged) in edits.iter().zip(staged) {
            let before = backup.files.iter().find(|file| &file.path == path).unwrap();
            write_engine::commit(staged, path, before.content.as_deref())?;
            committed.push((before, text));
        }
        s.active.insert(p.harness.clone(), p.id.clone());
        save(dir, &s)
    })();
    if let Err(error) = result {
        // Restore only our successful writes, and never overwrite a subsequent
        // external change while rolling back another file's failed commit.
        for (before, written) in committed.into_iter().rev() {
            if read(&before.path)?.as_deref() == Some(written.as_str()) {
                if let Some(original) = &before.content {
                    private_write(&before.path, original.as_bytes())?;
                } else {
                    std::fs::remove_file(&before.path)?;
                }
            }
        }
        return Err(error);
    }
    Ok(backup.id)
}

fn restore_files(backup: &Backup) -> Result<()> {
    for f in &backup.files {
        if let Some(text) = &f.content {
            private_write(&f.path, text.as_bytes())?;
        } else if f.path.exists() {
            std::fs::remove_file(&f.path)?;
        }
    }
    Ok(())
}
pub fn backup_harness(dir: &Path, id: &str) -> Result<String> {
    load(dir)?
        .backups
        .into_iter()
        .find(|b| b.id == id)
        .map(|b| b.harness)
        .ok_or_else(|| anyhow!("Backup not found"))
}
pub fn restore(dir: &Path, id: &str) -> Result<()> {
    let mut s = load(dir)?;
    let b = s
        .backups
        .iter()
        .find(|b| b.id == id)
        .cloned()
        .ok_or_else(|| anyhow!("Backup not found"))?;
    // Only the latest snapshot for a harness can be restored, avoiding stale rollback.
    if s.backups
        .iter()
        .rev()
        .find(|v| v.harness == b.harness)
        .map(|v| &v.id)
        != Some(&b.id)
    {
        bail!("Restore the latest backup for this harness first");
    }
    restore_files(&b)?;
    if let Some(active) = b.active {
        s.active.insert(b.harness.clone(), active);
    } else {
        s.active.remove(&b.harness);
        if b.harness == "grok" {
            s.grok_models.clear();
        }
    }
    s.backups.retain(|v| v.id != id);
    save(dir, &s)
}

pub async fn test_connection(p: &Profile) -> Result<Value> {
    validate(p, true)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let base = p.base_url.trim_end_matches('/');
    let (url, body) = match p.harness.as_str() {
        "claude" | "deepseek" if p.harness == "claude" || p.api_type == "anthropic" => (
            format!(
                "{base}/{}messages",
                if base.ends_with("/v1") { "" } else { "v1/" }
            ),
            json!({"model":p.model,"max_tokens":1,"messages":[{"role":"user","content":"Reply OK"}]}),
        ),
        "gemini" => (
            format!(
                "{base}/{}models/{}:generateContent",
                if base.ends_with("/v1beta") || base.ends_with("/v1") {
                    ""
                } else {
                    "v1beta/"
                },
                url::form_urlencoded::byte_serialize(p.model.as_bytes()).collect::<String>()
            ),
            json!({"contents":[{"parts":[{"text":"Reply OK"}]}],"generationConfig":{"maxOutputTokens":1}}),
        ),
        _ if p.api_type == "chat_completions" => (
            format!("{base}/chat/completions"),
            json!({"model":p.model,"messages":[{"role":"user","content":"Reply OK"}],"max_tokens":1}),
        ),
        _ => (
            format!("{base}/responses"),
            json!({"model":p.model,"input":"Reply OK","max_output_tokens":16}),
        ),
    };
    let request = authenticated(p, client.post(url).json(&body));
    let start = std::time::Instant::now();
    let body = response_json(request).await?;
    validate_model_response(p, &body)?;
    Ok(json!({"ok":true,"latencyMs":start.elapsed().as_millis(),"model":p.model}))
}

fn authenticated(p: &Profile, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let mut request = match p.harness.as_str() {
        "claude" if p.auth_type == "bearer" => request
            .bearer_auth(&p.api_key)
            .header("anthropic-version", "2023-06-01"),
        "claude" | "deepseek" if p.harness == "claude" || p.api_type == "anthropic" => request
            .header("x-api-key", &p.api_key)
            .header("anthropic-version", "2023-06-01"),
        "gemini" => request.header("x-goog-api-key", &p.api_key),
        _ => request.bearer_auth(&p.api_key),
    };
    for (name, value) in provider_config::headers(p) {
        request = request.header(name, value);
    }
    request
}

async fn response_json(request: reqwest::RequestBuilder) -> Result<Value> {
    let mut response = request.send().await.map_err(|_| {
        anyhow!("Connection failed or timed out. Check the URL and device network.")
    })?;
    let status = response.status();
    // Never echo an upstream response: it can contain credentials or HTML.
    if !status.is_success() {
        bail!(
            "Upstream returned HTTP {}. Check credentials, model and API compatibility.",
            status.as_u16()
        );
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("Unable to read upstream response"))?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            bail!("Unexpectedly large test response");
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("Upstream did not return a JSON model response"))?;
    if body.get("error").is_some_and(|v| !v.is_null()) {
        bail!("Upstream returned an API error. Check credentials, model and API compatibility.");
    }
    Ok(body)
}

fn validate_model_response(p: &Profile, body: &Value) -> Result<()> {
    let expected = match p.harness.as_str() {
        "claude" => "content",
        "deepseek" if p.api_type == "anthropic" => "content",
        "gemini" => "candidates",
        _ if p.api_type == "chat_completions" => "choices",
        _ => "output",
    };
    if body.get("error").is_some_and(|v| !v.is_null())
        || matches!(
            body.get("status").and_then(Value::as_str),
            Some("failed" | "cancelled")
        )
    {
        bail!("Upstream returned an API error. Check credentials, model and API compatibility.");
    }
    if !body.get(expected).is_some_and(Value::is_array) {
        bail!("Upstream response does not match the selected API format");
    }
    Ok(())
}

pub fn import_config(harness: &str, name: &str, text: &str, key: &str) -> Result<Profile> {
    if harness == "deepseek" {
        bail!("Import DSH using the current live configuration or its provider form");
    }
    // Accept native provider configuration or a CC Switch settingsConfig object.
    let parsed = serde_json::from_str::<Value>(text).ok();
    let wrapper = parsed
        .as_ref()
        .and_then(|v| v.get("settingsConfig"))
        .or(parsed.as_ref());
    let native = wrapper
        .and_then(|v| v.get("config"))
        .and_then(Value::as_str)
        .unwrap_or(text);
    let mut p = Profile {
        id: String::new(),
        name: name.into(),
        harness: harness.into(),
        base_url: String::new(),
        api_key: key.into(),
        auth_type: api_key_auth(),
        model: String::new(),
        api_type: responses(),
        context_window: context_window(),
        settings_config: empty_config(),
        sort_index: 0,
        revision: None,
        live_revision: None,
    };
    if matches!(harness, "codex" | "grok") {
        let doc = native
            .parse::<toml::Value>()
            .map_err(|_| anyhow!("Invalid TOML configuration"))?;
        let section = if harness == "codex" {
            let selected = doc
                .get("model_provider")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| anyhow!("Missing model_provider"))?;
            doc.get("model_providers").and_then(|v| v.get(selected))
        } else {
            let selected = doc
                .get("models")
                .and_then(|v| v.get("default"))
                .and_then(toml::Value::as_str)
                .ok_or_else(|| anyhow!("Missing default model"))?;
            doc.get("model").and_then(|v| v.get(selected))
        }
        .ok_or_else(|| anyhow!("Missing provider configuration"))?;
        p.base_url = section
            .get("base_url")
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .into();
        p.model = if harness == "codex" {
            doc.get("model")
        } else {
            section.get("model")
        }
        .and_then(toml::Value::as_str)
        .unwrap_or_default()
        .into();
        if p.api_key.is_empty() {
            p.api_key = section
                .get("api_key")
                .and_then(toml::Value::as_str)
                .or_else(|| {
                    wrapper
                        .and_then(|v| v.pointer("/auth/OPENAI_API_KEY"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .into();
        }
        if harness == "grok" {
            p.api_type = section
                .get("api_backend")
                .and_then(toml::Value::as_str)
                .unwrap_or("responses")
                .into();
            p.context_window = section
                .get("context_window")
                .and_then(toml::Value::as_integer)
                .unwrap_or(context_window());
        }
    } else {
        let doc = wrapper.ok_or_else(|| anyhow!("Expected a JSON settings object"))?;
        let env = doc.get("env").unwrap_or(doc);
        if harness == "claude"
            && env
                .get("ANTHROPIC_AUTH_TOKEN")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
        {
            p.auth_type = "bearer".into();
        }
        let (url, secret, model) = if harness == "claude" {
            (
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_MODEL",
            )
        } else {
            ("GOOGLE_GEMINI_BASE_URL", "GEMINI_API_KEY", "GEMINI_MODEL")
        };
        p.base_url = env[url]
            .as_str()
            .unwrap_or(if harness == "gemini" {
                "https://generativelanguage.googleapis.com"
            } else {
                "https://api.anthropic.com"
            })
            .into();
        p.model = env[model]
            .as_str()
            .or_else(|| doc.get("model").and_then(Value::as_str))
            .or_else(|| doc.pointer("/model/name").and_then(Value::as_str))
            .unwrap_or_default()
            .into();
        if p.api_key.is_empty() {
            p.api_key = env[secret]
                .as_str()
                .filter(|key| !key.trim().is_empty())
                .or_else(|| {
                    env[if harness == "gemini" {
                        "GOOGLE_API_KEY"
                    } else {
                        "ANTHROPIC_API_KEY"
                    }]
                    .as_str()
                })
                .unwrap_or_default()
                .into();
        }
    }
    if harness == "claude" {
        if let Some(doc) = wrapper.and_then(Value::as_object) {
            for (key, value) in doc {
                if cc_switch::claude_provider_top(key) {
                    p.settings_config[key] = value.clone();
                }
            }
            if let Some(env) = doc.get("env").and_then(Value::as_object) {
                p.settings_config["env"] = json!({});
                for (key, value) in env {
                    if cc_switch::claude_provider_env(key) {
                        p.settings_config["env"][key] = value.clone();
                    }
                }
            }
        }
    } else if harness == "gemini" {
        p.settings_config["env"] = json!({});
        if let Some(env) = wrapper.and_then(|v| v.get("env").unwrap_or(v).as_object()) {
            for (key, value) in env {
                if cc_switch::gemini_provider_env(key) {
                    p.settings_config["env"][key] = value.clone();
                }
            }
        }
    } else if harness == "codex" {
        let doc: toml::Value = native.parse()?;
        for (key, value) in doc.as_table().unwrap() {
            if cc_switch::codex_provider_top(key) {
                p.settings_config[key] = serde_json::to_value(value)?;
            }
        }
        if let Some(selected) = doc.get("model_provider").and_then(toml::Value::as_str) {
            if let Some(provider) = doc.get("model_providers").and_then(|v| v.get(selected)) {
                p.settings_config["provider"] = serde_json::to_value(provider)?;
            }
        }
    }
    validate(&p, true)?;
    Ok(p)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Template {
    pub schema_version: u32,
    #[serde(default)]
    pub harnesses: Vec<String>,
    pub profiles: Vec<Profile>,
}
pub fn parse_template(value: Value) -> Result<Template> {
    let t: Template = serde_json::from_value(value).map_err(|_| {
        anyhow!("Expected a Pockymoe template with schemaVersion, harnesses and profiles")
    })?;
    if t.schema_version != 1 || t.profiles.len() > 50 || t.harnesses.len() > 10 {
        bail!("Unsupported template version or too many entries");
    }
    let mut active = std::collections::HashSet::new();
    for p in &t.profiles {
        let mut candidate = p.clone();
        if candidate.model.trim().is_empty() {
            candidate.model = "__auto_detect__".into();
        }
        validate(&candidate, true)?;
        if !active.insert(&p.harness) {
            bail!("A template may activate only one profile per harness");
        }
    }
    for id in &t.harnesses {
        if !crate::management::can_install(id) {
            bail!("Unsupported harness installation: {id}");
        }
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleting_active_upstream_restores_original_config_and_keeps_other_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("grok");
        let store = temp.path().join("profiles");
        let original = "# original\n[ui]\ncolor = 'blue'\n";
        private_write(&root.join("config.toml"), original.as_bytes()).unwrap();
        let first = fixture("grok");
        let second = fixture("grok");
        for p in [&first, &second] {
            upsert(&store, p.clone()).unwrap();
            activate_at(&store, p, &root).unwrap();
        }
        remove(&store, &second.id).unwrap();
        assert_eq!(read(&root.join("config.toml")).unwrap().unwrap(), original);
        assert!(active_profile(&store, "grok").unwrap().is_none());
        assert!(profile(&store, &first.id).is_ok());
        assert!(profile(&store, &second.id).is_err());
        assert!(load(&store).unwrap().backups.is_empty());
        assert!(load(&store).unwrap().grok_models.is_empty());
    }

    #[test]
    fn grok_catalog_maps_every_model_to_the_same_upstream_without_overwriting_native_entries() {
        let temp = tempfile::tempdir().unwrap();
        private_write(
            &temp.path().join("config.toml"),
            b"[model.personal]\nmodel='keep'\n[model.'remote-codex/obsolete']\nmodel='obsolete'\n",
        )
        .unwrap();
        let p = fixture("grok");
        let models = ["model-a", "model-b"].map(|id| DiscoveredModel {
            id: id.into(),
            name: id.into(),
            reasoning: None,
        });
        prepare_grok_models_at(&temp.path().join("store"), &p, &models, temp.path()).unwrap();
        let doc = toml_doc(&temp.path().join("config.toml")).unwrap();
        assert_eq!(doc["model"]["personal"]["model"].as_str(), Some("keep"));
        assert!(doc["model"].get("remote-codex/obsolete").is_none());
        for id in ["model-a", "model-b"] {
            assert_eq!(
                doc["model"][id]["api_key"].as_str(),
                Some(p.api_key.as_str())
            );
            let configured = &doc["model"][&format!("remote-codex/{id}")];
            assert_eq!(configured["model"].as_str(), Some(id));
            assert_eq!(configured["name"].as_str(), Some(id));
            assert_eq!(configured["base_url"].as_str(), Some(p.base_url.as_str()));
            assert_eq!(configured["api_key"].as_str(), Some(p.api_key.as_str()));
        }
    }
    #[test]
    fn grok_native_overrides_restore_originals_across_refreshes_and_switches() {
        let original = "[model.'grok-4.6']\napi_key='original'\nenv_key='XAI_API_KEY'\nauth_provider='native'\nextra_headers={Authorization='old'}\nreasoning_efforts=[{value='high',default=true}]\n[model.personal]\nmodel='keep'\n";
        let mut doc: DocumentMut = original.parse().unwrap();
        let mut originals = BTreeMap::new();
        let mut p = fixture("grok");
        p.model = "grok-4.6".into();
        configure_grok_models(&mut doc, &p, ["grok-4.5"].into_iter(), &mut originals).unwrap();
        // Exercise the persisted representation, not just an in-memory table.
        let saved: BTreeMap<String, Option<String>> =
            serde_json::from_str(&serde_json::to_string(&originals).unwrap()).unwrap();
        originals = saved;
        doc = doc.to_string().parse().unwrap();
        for _ in 0..2 {
            configure_grok_models(&mut doc, &p, ["grok-4.5"].into_iter(), &mut originals).unwrap();
            let model = &doc["model"]["grok-4.6"];
            assert_eq!(model["api_key"].as_str(), Some(p.api_key.as_str()));
            assert!(model.get("env_key").is_none());
            assert!(model.get("auth_provider").is_none());
            assert!(model.get("extra_headers").is_none());
            assert!(model.get("reasoning_efforts").is_some());
        }
        p.model = "grok-4.5".into();
        p.api_key = "second-key".into();
        configure_grok_models(&mut doc, &p, std::iter::empty(), &mut originals).unwrap();
        assert_eq!(
            doc["model"]["grok-4.6"]["api_key"].as_str(),
            Some("original")
        );
        assert_eq!(
            doc["model"]["grok-4.6"]["env_key"].as_str(),
            Some("XAI_API_KEY")
        );
        assert!(doc["model"].get("remote-codex/grok-4.6").is_none());
        assert_eq!(
            doc["model"]["grok-4.5"]["api_key"].as_str(),
            Some("second-key")
        );
        p.model = "new-model".into();
        configure_grok_models(&mut doc, &p, std::iter::empty(), &mut originals).unwrap();
        assert!(doc["model"].get("grok-4.5").is_none());
        assert_eq!(doc["model"]["personal"]["model"].as_str(), Some("keep"));
        assert_eq!(doc["models"]["default"].as_str(), Some("new-model"));
    }
    #[test]
    fn grok_discovered_capabilities_refresh_both_ids_and_restore_originals() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("store");
        let config = temp.path().join("config.toml");
        let original = "[model.'grok-build-0.1']\nreasoning_effort='xhigh'\nsupports_reasoning_effort=true\nreasoning_efforts=[{value='xhigh',default=true}]\n";
        private_write(&config, original.as_bytes()).unwrap();
        let mut p = fixture("grok");
        p.model = "grok-build-0.1".into();
        upsert(&store, p.clone()).unwrap();
        activate_at(&store, &p, temp.path()).unwrap();
        let mut model: DiscoveredModel = serde_json::from_value(json!({"id":p.model,"name":"Build","reasoning":{
            "efforts":[{"reasoningEffort":"low","description":"Low"},{"reasoningEffort":"high","description":"High"}],"default":"high"
        }})).unwrap();
        prepare_grok_models_at(&store, &p, &[model.clone()], temp.path()).unwrap();
        let doc = toml_doc(&config).unwrap();
        for id in ["grok-build-0.1", "remote-codex/grok-build-0.1"] {
            let entry = &doc["model"][id];
            assert_eq!(entry["reasoning_efforts"].as_array().unwrap().len(), 2);
            assert!(entry.get("reasoning_effort").is_none());
        }
        model.reasoning.as_mut().unwrap().efforts.clear();
        model.reasoning.as_mut().unwrap().default = None;
        prepare_grok_models_at(&store, &p, &[model.clone()], temp.path()).unwrap();
        let doc = toml_doc(&config).unwrap();
        assert_eq!(
            doc["model"][&p.model]["supports_reasoning_effort"].as_bool(),
            Some(false)
        );
        model.reasoning = None;
        prepare_grok_models_at(&store, &p, &[model], temp.path()).unwrap();
        let doc = toml_doc(&config).unwrap();
        assert_eq!(
            doc["model"][&p.model]["reasoning_effort"].as_str(),
            Some("xhigh")
        );
        assert!(doc["model"]["remote-codex/grok-build-0.1"]
            .get("reasoning_efforts")
            .is_none());
        remove(&store, &p.id).unwrap();
        assert_eq!(read(&config).unwrap().unwrap(), original);
    }
    #[test]
    fn connection_accepts_null_error_but_rejects_errors_and_wrong_protocol() {
        let p = fixture("grok");
        assert!(validate_model_response(
            &p,
            &json!({"status":"completed","error":null,"output":[]})
        )
        .is_ok());
        for body in [
            json!({"output":[],"error":{"message":"failed"}}),
            json!({"output":[],"status":"failed"}),
            json!({"output":[],"status":"cancelled"}),
            json!({"choices":[]}),
        ] {
            assert!(validate_model_response(&p, &body).is_err());
        }
    }
    pub(super) fn fixture(harness: &str) -> Profile {
        Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "Test".into(),
            harness: harness.into(),
            base_url: "https://example.com/v1".into(),
            api_key: "secret-fixture".into(),
            auth_type: api_key_auth(),
            model: "test-model".into(),
            api_type: responses(),
            context_window: context_window(),
            settings_config: empty_config(),
            sort_index: 0,
            revision: None,
            live_revision: None,
        }
    }
    #[test]
    fn switch_preserves_other_settings_and_restores_exact_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("codex");
        let store = dir.path().join("profiles");
        let original="# Keep comment\n[projects.test]\ntrust_level = 'trusted'\n[mcp_servers.example]\ncommand = 'example'\n";
        private_write(&root.join("config.toml"), original.as_bytes()).unwrap();
        let p = fixture("codex");
        upsert(&store, p.clone()).unwrap();
        let backup = activate_at(&store, &p, &root).unwrap();
        let text = read(&root.join("config.toml")).unwrap().unwrap();
        assert!(text.contains("# Keep comment"));
        assert!(text.contains("mcp_servers.example"));
        assert!(!inventory(&store)
            .unwrap()
            .to_string()
            .contains("secret-fixture"));
        restore(&store, &backup).unwrap();
        assert_eq!(read(&root.join("config.toml")).unwrap().unwrap(), original);
        assert!(!root.join("auth.json").exists());
    }
    #[test]
    fn changed_destination_never_reuses_saved_secret() {
        let dir = tempfile::tempdir().unwrap();
        let p = fixture("codex");
        upsert(dir.path(), p.clone()).unwrap();
        let mut edited = p;
        edited.api_key.clear();
        edited.base_url = "https://different.example/v1".into();
        assert!(upsert(dir.path(), edited).is_err());
    }
    #[test]
    fn provider_files_are_scoped_and_invalid_config_is_preserved() {
        for id in ["claude", "gemini", "grok"] {
            let dir = tempfile::tempdir().unwrap();
            let p = fixture(id);
            let edits = changes(&p, dir.path()).unwrap();
            assert!(!edits.is_empty());
            assert!(edits.iter().all(|(path, _)| path.starts_with(dir.path())));
        }
        let dir = tempfile::tempdir().unwrap();
        private_write(&dir.path().join("config.toml"), b"broken = [").unwrap();
        assert!(changes(&fixture("codex"), dir.path()).is_err());
        for (harness, content) in [
            ("grok", "model = 'bad'"),
            ("codex", "model_providers = 5"),
            ("codex", "model_providers = {remote_codex = 5}"),
        ] {
            private_write(&dir.path().join("config.toml"), content.as_bytes()).unwrap();
            assert!(changes(&fixture(harness), dir.path()).is_err());
            assert_eq!(
                read(&dir.path().join("config.toml")).unwrap().unwrap(),
                content
            );
        }
    }
    #[test]
    fn claude_import_preserves_authentication_mode_and_unrelated_settings() {
        for (key, mode, other) in [
            ("ANTHROPIC_API_KEY", "api_key", "ANTHROPIC_AUTH_TOKEN"),
            ("ANTHROPIC_AUTH_TOKEN", "bearer", "ANTHROPIC_API_KEY"),
        ] {
            let text =
                json!({"settingsConfig":{"env":{key:"synthetic",other:"", "ANTHROPIC_MODEL":"test"}}})
                    .to_string();
            let p = import_config("claude", "Imported", &text, "").unwrap();
            assert_eq!(p.auth_type, mode);
            let dir = tempfile::tempdir().unwrap();
            private_write(
                &dir.path().join("settings.json"),
                json!({"env":{other:"old"},"permissions":{"allow":["Read"]}})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
            let edits = changes(&p, dir.path()).unwrap();
            let doc: Value = serde_json::from_str(&edits[0].1).unwrap();
            assert_eq!(doc["env"][key], "synthetic");
            assert!(doc["env"].get(other).is_none());
            assert_eq!(doc["permissions"]["allow"][0], "Read");
        }
    }
    #[test]
    fn claude_gateway_tuning_backfills_and_returns_with_its_provider() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("claude");
        let store = temp.path().join("store");
        let original = json!({"env":{"ANTHROPIC_CUSTOM_HEADERS":"x-gateway-auth: native","ANTHROPIC_DEFAULT_HAIKU_MODEL":"native-haiku"},"modelOverrides":{"alias":"native"},"fallbackModel":"native-fallback"}).to_string();
        private_write(&root.join("settings.json"), original.as_bytes()).unwrap();
        let a = fixture("claude");
        let b = fixture("claude");
        upsert(&store, a.clone()).unwrap();
        upsert(&store, b.clone()).unwrap();
        activate_at(&store, &a, &root).unwrap();
        let mut live = json_doc(&root.join("settings.json")).unwrap();
        assert_eq!(
            live["env"]["ANTHROPIC_CUSTOM_HEADERS"],
            "x-gateway-auth: native"
        );
        live["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"] = json!("manually-edited-sonnet");
        live["env"]["CLAUDE_CODE_SUBAGENT_MODEL"] = json!("manual-subagent");
        private_write(
            &root.join("settings.json"),
            serde_json::to_string(&live).unwrap().as_bytes(),
        )
        .unwrap();
        activate_at(&store, &b, &root).unwrap();
        assert!(json_doc(&root.join("settings.json")).unwrap()["env"]
            .get("ANTHROPIC_CUSTOM_HEADERS")
            .is_none());
        activate_at(&store, &profile(&store, &a.id).unwrap(), &root).unwrap();
        let restored = json_doc(&root.join("settings.json")).unwrap();
        for key in [
            "ANTHROPIC_CUSTOM_HEADERS",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            assert_eq!(restored["env"][key], live["env"][key]);
        }
        assert_eq!(restored["modelOverrides"], live["modelOverrides"]);
        assert_eq!(restored["fallbackModel"], live["fallbackModel"]);
    }
    #[test]
    fn claude_switch_never_removes_tool_environment_or_user_settings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("claude");
        let store = temp.path().join("store");
        let original = json!({"env":{"AWS_PROFILE":"staging","AWS_REGION":"ca-central-1","GOOGLE_APPLICATION_CREDENTIALS":"/isolated/google.json","CLOUD_ML_REGION":"test","CLAUDE_CODE_USE_NATIVE_FILE_SEARCH":"1","USER_SETTING":"keep"},"hooks":{"SessionStart":[]},"permissions":{"allow":["Read"]},"apiKeyHelper":"user-helper"});
        private_write(
            &root.join("settings.json"),
            serde_json::to_vec(&original).unwrap().as_slice(),
        )
        .unwrap();
        for _ in 0..3 {
            let p = fixture("claude");
            upsert(&store, p.clone()).unwrap();
            activate_at(&store, &p, &root).unwrap();
        }
        let live = json_doc(&root.join("settings.json")).unwrap();
        for (key, value) in original["env"].as_object().unwrap() {
            assert_eq!(&live["env"][key], value);
        }
        for key in ["hooks", "permissions", "apiKeyHelper"] {
            assert_eq!(live[key], original[key]);
        }
    }
    #[test]
    fn codex_and_gemini_strip_outgoing_provider_knobs_and_backfill_them() {
        for harness in ["codex", "gemini"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(harness);
            let store = temp.path().join("store");
            let mut a = fixture(harness);
            let b = fixture(harness);
            a.settings_config = if harness == "codex" {
                json!({"model_reasoning_effort":"high","review_model":"review-a","experimental_bearer_token":"test-token","provider":{"http_headers":{"x-gateway":"a"}}})
            } else {
                json!({"env":{"GOOGLE_API_KEY":"test-google-key","GEMINI_CLI_CUSTOM_HEADERS":"x-gateway: a"}})
            };
            upsert(&store, a.clone()).unwrap();
            upsert(&store, b.clone()).unwrap();
            activate_at(&store, &a, &root).unwrap();
            activate_at(&store, &b, &root).unwrap();
            let saved = profile(&store, &a.id).unwrap();
            if harness == "codex" {
                let live = toml_doc(&root.join("config.toml")).unwrap();
                for key in [
                    "model_reasoning_effort",
                    "review_model",
                    "experimental_bearer_token",
                ] {
                    assert!(live.get(key).is_none());
                    assert!(saved.settings_config.get(key).is_some());
                }
            } else {
                let live = read(&root.join(".env")).unwrap().unwrap();
                assert!(!live.contains("GOOGLE_API_KEY="));
                assert!(!live.contains("GEMINI_CLI_CUSTOM_HEADERS="));
                assert_eq!(
                    saved.settings_config["env"]["GOOGLE_API_KEY"],
                    "test-google-key"
                );
            }
            activate_at(&store, &saved, &root).unwrap();
            if harness == "codex" {
                assert_eq!(
                    toml_doc(&root.join("config.toml")).unwrap()["review_model"].as_str(),
                    Some("review-a")
                );
            } else {
                assert!(read(&root.join(".env"))
                    .unwrap()
                    .unwrap()
                    .contains("GOOGLE_API_KEY="));
            }
        }
    }
    #[test]
    fn templates_reject_commands_and_insecure_destinations() {
        let mut p = fixture("codex");
        p.base_url = "http://remote.example/v1".into();
        assert!(validate(&p, true).is_err());
        assert!(
            parse_template(json!({"schemaVersion":1,"profiles":[],"commands":["echo bad"]}))
                .is_err()
        );
    }
}
#[test]
fn templates_allow_model_omission_for_upstream_discovery() {
    let template = parse_template(json!({
        "schemaVersion": 1,
        "harnesses": ["codex"],
        "profiles": [{
            "name": "Gateway",
            "harness": "codex",
            "baseUrl": "https://gateway.example/v1",
            "apiKey": "secret",
            "apiType": "responses"
        }]
    }))
    .unwrap();
    assert_eq!(template.profiles[0].model, "");
}

/// Read native provider parameters; callers redact before returning over HTTP.
fn read_live_profile(harness: &str, name: &str, root: &Path) -> Result<Profile> {
    if harness == "deepseek" {
        return dsh::import_live(root, name);
    }
    let native = if harness == "codex" {
        json!({"config":read(&root.join("config.toml"))?.unwrap_or_default(),"auth":json_doc(&root.join("auth.json"))?}).to_string()
    } else if harness == "grok" {
        read(&root.join("config.toml"))?.unwrap_or_default()
    } else if harness == "claude" {
        json_doc(&root.join("settings.json"))?.to_string()
    } else {
        provider_config::capture(harness, root)?.to_string()
    };
    let mut p = import_config(harness, name, &native, "")?;
    p.settings_config = provider_config::capture(harness, root)?;
    Ok(p)
}
fn live_revision(p: &Profile, root: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let files = native_paths(p, root)?
        .iter()
        .map(|path| read(path))
        .collect::<Result<Vec<_>>>()?;
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&files)?)))
}
pub fn inspect_profile(dir: &Path, id: &str) -> Result<Value> {
    let p = profile(dir, id)?;
    inspect_profile_at(dir, id, &home(&p.harness))
}
fn inspect_profile_at(dir: &Path, id: &str, root: &Path) -> Result<Value> {
    let mut p = profile(dir, id)?;
    if active_profile(dir, &p.harness)?.is_some_and(|active| active.id == id) {
        let before = live_revision(&p, root)?;
        p.settings_config = provider_config::capture(&p.harness, root)?;
        if let Ok(live) = read_live_profile(&p.harness, &p.name, root) {
            p.base_url = live.base_url;
            p.model = live.model;
            p.api_key = live.api_key;
            p.auth_type = live.auth_type;
            p.api_type = live.api_type;
        }
        if before != live_revision(&p, root)? {
            bail!(write_engine::CONFLICT);
        }
        let mut result = redacted(&p);
        result["revision"] = json!(provider_config::revision(&profile(dir, id)?));
        result["liveRevision"] = json!(before);
        return Ok(result);
    }
    Ok(redacted(&p))
}
pub fn save_and_apply(dir: &Path, p: Profile) -> Result<Value> {
    let root = home(&p.harness);
    save_and_apply_at(dir, p, &root)
}
fn save_and_apply_at(dir: &Path, p: Profile, root: &Path) -> Result<Value> {
    let original = load(dir)?;
    let active = original.active.get(&p.harness) == Some(&p.id);
    let expected = p.live_revision.clone();
    let saved = upsert_at(dir, p, root)?;
    if active {
        let mut target = profile(dir, saved["id"].as_str().unwrap())?;
        target.live_revision = expected;
        if let Err(error) = activate_at(dir, &target, root) {
            save(dir, &original)?;
            return Err(error);
        }
    }
    Ok(saved)
}
pub fn import_live(dir: &Path, harness: &str, name: &str) -> Result<Value> {
    upsert(dir, read_live_profile(harness, name, &home(harness))?)
}
pub fn reorder(dir: &Path, id: &str, direction: &str) -> Result<()> {
    let mut store = load(dir)?;
    let harness = store
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("Profile not found"))?
        .harness
        .clone();
    let mut rows = store
        .profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| p.harness == harness)
        .map(|(i, p)| (i, p.sort_index))
        .collect::<Vec<_>>();
    rows.sort_by_key(|(_, sort)| *sort);
    let index = rows
        .iter()
        .position(|(i, _)| store.profiles[*i].id == id)
        .unwrap();
    let target = match direction {
        "up" => index.saturating_sub(1),
        "down" => (index + 1).min(rows.len() - 1),
        _ => bail!("Unknown reorder direction"),
    };
    rows.swap(index, target);
    for (sort, (i, _)) in rows.into_iter().enumerate() {
        store.profiles[i].sort_index = sort as i64;
    }
    save(dir, &store)
}
pub async fn endpoint_speed(p: &Profile) -> Result<Value> {
    validate(p, true)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let start = std::time::Instant::now();
    let suffix = if p.harness == "gemini" {
        "v1beta/models"
    } else if p.harness == "claude" || p.api_type == "anthropic" {
        "v1/models"
    } else {
        "models"
    };
    let base = p.base_url.trim_end_matches('/');
    let suffix = if base.ends_with("/v1") || base.ends_with("/v1beta") {
        "models"
    } else {
        suffix
    };
    let response = authenticated(p, client.get(format!("{base}/{suffix}")))
        .send()
        .await
        .map_err(|_| anyhow!("Endpoint speed test failed or timed out"))?;
    Ok(json!({"latencyMs":start.elapsed().as_millis(),"status":response.status().as_u16()}))
}

#[cfg(test)]
mod provider_edit_tests {
    use super::*;
    #[test]
    fn stale_provider_editor_is_rejected_and_private_fragments_stay_private() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let mut p = tests::fixture("claude");
        p.settings_config =
            json!({"env":{"ANTHROPIC_CUSTOM_HEADERS":"x-gateway-token: synthetic-private"}});
        let mut view = upsert(dir, p.clone()).unwrap();
        assert!(!view.to_string().contains("synthetic-private"));
        view.as_object_mut().unwrap().remove("hasApiKey");
        let old: Profile = serde_json::from_value(view.clone()).unwrap();
        p.name = "Other edit".into();
        upsert(dir, p).unwrap();
        assert!(upsert(dir, old)
            .unwrap_err()
            .to_string()
            .contains("edit conflict"));
        view = inventory(dir).unwrap()["profiles"][0].clone();
        view.as_object_mut().unwrap().remove("hasApiKey");
        let mut current: Profile = serde_json::from_value(view).unwrap();
        current.name = "My edit".into();
        upsert(dir, current.clone()).unwrap();
        assert_eq!(
            profile(dir, &current.id).unwrap().settings_config["env"]["ANTHROPIC_CUSTOM_HEADERS"],
            "x-gateway-token: synthetic-private"
        );
    }
    #[test]
    fn duplicating_redacted_fragments_never_writes_secret_placeholders() {
        let temp = tempfile::tempdir().unwrap();
        for harness in ["claude", "deepseek"] {
            let mut p = tests::fixture(harness);
            p.settings_config = if harness == "claude" {
                json!({"env":{"ANTHROPIC_CUSTOM_HEADERS":"X-Key: original-private"}})
            } else {
                json!({"headers":{"X-Key":"original-private"}})
            };
            let mut view = upsert(temp.path(), p).unwrap();
            view.as_object_mut().unwrap().remove("hasApiKey");
            let mut copy: Profile = serde_json::from_value(view).unwrap();
            copy.id.clear();
            copy.revision = None;
            copy.api_key = "synthetic-new-key".into();
            let copied = upsert(temp.path(), copy).unwrap();
            let stored = profile(temp.path(), copied["id"].as_str().unwrap()).unwrap();
            assert!(!stored
                .settings_config
                .to_string()
                .contains("[stored privately]"));
            assert!(!stored
                .settings_config
                .to_string()
                .contains("original-private"));
        }
    }
    #[test]
    fn first_explicit_preset_adopts_unowned_tuning_and_active_edit_reapplies_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("claude");
        let dir = temp.path().join("store");
        private_write(&root.join("settings.json"),br#"{"env":{"ANTHROPIC_DEFAULT_HAIKU_MODEL":"custom-haiku","AWS_PROFILE":"tool-account"},"modelOverrides":{"sonnet":"custom-sonnet"}}"#).unwrap();
        let mut p = tests::fixture("claude");
        p.settings_config = json!({"env":{"ANTHROPIC_DEFAULT_OPUS_MODEL":"preset-opus"}});
        upsert(&dir, p.clone()).unwrap();
        activate_at(&dir, &p, &root).unwrap();
        let mut saved = profile(&dir, &p.id).unwrap();
        assert_eq!(
            saved.settings_config["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"],
            "custom-haiku"
        );
        saved.settings_config["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"] = json!("edited-opus");
        save_and_apply_at(&dir, saved, &root).unwrap();
        let doc = json_doc(&root.join("settings.json")).unwrap();
        assert_eq!(doc["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "edited-opus");
        assert_eq!(doc["env"]["AWS_PROFILE"], "tool-account");
    }
    #[test]
    fn active_editor_preserves_live_tuning_and_rejects_changes_after_opening() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("claude");
        let dir = temp.path().join("store");
        let p = tests::fixture("claude");
        upsert(&dir, p.clone()).unwrap();
        activate_at(&dir, &p, &root).unwrap();
        let path = root.join("settings.json");
        let mut doc = json_doc(&path).unwrap();
        doc["env"]["ANTHROPIC_CUSTOM_HEADERS"] = json!("X-Key: synthetic-live-secret");
        doc["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"] = json!("live-haiku");
        doc["env"]["ANTHROPIC_API_KEY"] = json!("synthetic-manually-rotated-key");
        private_write(&path, doc.to_string().as_bytes()).unwrap();
        let mut view = inspect_profile_at(&dir, &p.id, &root).unwrap();
        assert!(!view.to_string().contains("synthetic-live-secret"));
        view.as_object_mut().unwrap().remove("hasApiKey");
        let mut editor: Profile = serde_json::from_value(view).unwrap();
        editor.name = "Renamed".into();
        save_and_apply_at(&dir, editor, &root).unwrap();
        assert_eq!(
            json_doc(&path).unwrap()["env"]["ANTHROPIC_CUSTOM_HEADERS"],
            "X-Key: synthetic-live-secret"
        );
        assert_eq!(
            profile(&dir, &p.id).unwrap().api_key,
            "synthetic-manually-rotated-key"
        );
        assert_eq!(
            json_doc(&path).unwrap()["env"]["ANTHROPIC_API_KEY"],
            "synthetic-manually-rotated-key"
        );
        let mut stale = inspect_profile_at(&dir, &p.id, &root).unwrap();
        stale.as_object_mut().unwrap().remove("hasApiKey");
        doc = json_doc(&path).unwrap();
        doc["newExternalSetting"] = json!(true);
        private_write(&path, doc.to_string().as_bytes()).unwrap();
        let before = read(&path).unwrap();
        let old_name = profile(&dir, &p.id).unwrap().name;
        let error =
            save_and_apply_at(&dir, serde_json::from_value(stale).unwrap(), &root).unwrap_err();
        assert!(error.to_string().contains("Native configuration changed"));
        assert_eq!(read(&path).unwrap(), before);
        assert_eq!(profile(&dir, &p.id).unwrap().name, old_name);
    }
    #[test]
    fn ordering_is_persistent_and_scoped_to_harness() {
        let temp = tempfile::tempdir().unwrap();
        let a = tests::fixture("codex");
        let b = tests::fixture("codex");
        let c = tests::fixture("claude");
        for p in [&a, &b, &c] {
            upsert(temp.path(), p.clone()).unwrap();
        }
        reorder(temp.path(), &b.id, "up").unwrap();
        let rows = inventory(temp.path()).unwrap();
        let rows = rows["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["harness"] == "codex")
            .collect::<Vec<_>>();
        assert_eq!(rows[0]["id"], b.id);
        assert_eq!(rows[1]["id"], a.id);
        assert_eq!(profile(temp.path(), &c.id).unwrap().sort_index, 0);
    }
}
