//! Harness-local native authentication/configuration. Never an API connection.
//! Only provider-owned fields are retained; login files stay in place, except
//! Codex's overwritten auth fields, which need a private fallback snapshot.
use super::*;
use anyhow::ensure;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

pub const HARNESSES: [&str; 5] = ["codex", "claude", "gemini", "grok", "deepseek"];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NativeState {
    root: PathBuf,
    fragment: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth: Option<Value>,
    kind: String,
    generation: String,
    // Discovery is read-only with respect to child-process behavior. Only a
    // successful explicit activation authorizes suppressing inherited API env.
    #[serde(default)]
    selected: bool,
}

fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.trim().is_empty())
}
fn expiry(v: &Value) -> Option<i64> {
    v.as_i64()
        .map(|n| if n > 10_000_000_000 { n / 1000 } else { n })
        .or_else(|| {
            v.as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp())
        })
}
fn jwt_exp(token: &Value) -> Option<i64> {
    let payload = token.as_str()?.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    value["exp"].as_i64()
}
fn oauth(access: &Value, refresh: &Value, expires: &Value) -> bool {
    let now = Utc::now().timestamp();
    let refreshable = nonempty(refresh) && jwt_exp(refresh).is_none_or(|exp| exp > now);
    refreshable
        || (nonempty(access)
            && expiry(expires)
                .or_else(|| jwt_exp(access))
                .is_some_and(|exp| exp > now))
}
fn credentials(path: &Path) -> Result<Value> {
    let Some(raw) = read(path)? else {
        return Ok(Value::Null);
    };
    serde_json::from_str(&raw).map_err(|_| anyhow!("Native authentication file is invalid"))
}
fn claude_credentials(root: &Path) -> Result<Value> {
    let value = credentials(&root.join(".credentials.json"))?;
    if !value.is_null() {
        return Ok(value);
    }
    // The CLI uses the OS keychain only for its default config directory. Never
    // probe it in tests or for explicitly supplied fixture/custom directories.
    #[cfg(all(target_os = "macos", not(test)))]
    if root == crate::config::home_dir().join(".claude")
        && std::env::var_os("CLAUDE_CONFIG_DIR").is_none()
    {
        let mut command = std::process::Command::new("security");
        crate::child_process::hide_std(&mut command);
        command.args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ]);
        if let Some(output) = bounded_output(&mut command, std::time::Duration::from_millis(1500)) {
            return serde_json::from_slice(&output)
                .map_err(|_| anyhow!("Native authentication is invalid"));
        }
    }
    Ok(Value::Null)
}
// A locked keychain must not indefinitely hold the upstream gate. Private
// temporary output avoids pipe-buffer deadlocks and caps accepted output size.
#[cfg(any(target_os = "macos", test))]
fn bounded_output(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Option<Vec<u8>> {
    use std::{io::Read, process::Stdio, time::Instant};
    let output = tempfile::NamedTempFile::new().ok()?;
    let mut child = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::from(output.reopen().ok()?))
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut bytes = Vec::new();
                std::fs::File::open(output.path())
                    .ok()?
                    .take(65537)
                    .read_to_end(&mut bytes)
                    .ok()?;
                return (bytes.len() <= 65536).then_some(bytes);
            }
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
fn auth_fragment(value: &Value) -> Value {
    let mut out = json!({});
    for key in ["auth_mode", "OPENAI_API_KEY", "tokens", "last_refresh"] {
        if let Some(value) = value.get(key) {
            out[key] = value.clone();
        }
    }
    out
}
fn codex_oauth(value: &Value) -> bool {
    oauth(
        &value["tokens"]["access_token"],
        &value["tokens"]["refresh_token"],
        &value["tokens"]["expires_at"],
    )
}
fn codex_auth(root: &Path, state: &NativeState) -> Result<Value> {
    let current = credentials(&root.join("auth.json"))?;
    // A harness/another client may refresh or replace the account while an API
    // is selected. Never put older refresh tokens over usable newer tokens.
    let mut auth = if codex_oauth(&current) && state.kind == "oauth" {
        auth_fragment(&current)
    } else {
        state.auth.clone().unwrap_or(json!({}))
    };
    if state.kind == "oauth" {
        auth["auth_mode"] = json!("chatgpt");
        auth["OPENAI_API_KEY"] = Value::Null;
    }
    Ok(auth)
}
fn grok_auth(value: &Value) -> Option<&'static str> {
    value.as_object()?.values().find_map(|v| {
        if v["auth_mode"] == "api_key" && nonempty(&v["key"]) {
            return Some("config");
        }
        if v["auth_mode"] == "external" {
            return oauth(&v["key"], &Value::Null, &v["expires_at"]).then_some("oauth");
        }
        if v["auth_mode"] != "oidc" {
            return None;
        }
        let expires =
            expiry(&v["expires_at"]).or_else(|| expiry(&v["create_time"]).map(|t| t + 30 * 86400));
        let refresh = if nonempty(&v["issuer"]) {
            &v["refresh_token"]
        } else {
            &Value::Null
        };
        oauth(&v["key"], refresh, &json!(expires)).then_some("oauth")
    })
}
fn gemini_adc(root: &Path) -> Result<bool> {
    // This path is user-owned/global within Gemini and is not projected by API
    // switches. Check the live reference rather than persisting a stale path.
    let env = provider_config::dotenv(&read(&root.join(".env"))?.unwrap_or_default());
    let path = env
        .into_iter()
        .rev()
        .find_map(|(key, value)| (key == "GOOGLE_APPLICATION_CREDENTIALS").then_some(value))
        .or_else(|| std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok());
    let Some(path) = path else {
        return Ok(false);
    };
    let c = credentials(Path::new(&path))?;
    Ok(match c["type"].as_str() {
        Some("service_account") => nonempty(&c["client_email"]) && nonempty(&c["private_key"]),
        Some("authorized_user") => {
            nonempty(&c["client_id"])
                && nonempty(&c["client_secret"])
                && nonempty(&c["refresh_token"])
        }
        _ => false,
    })
}
fn usable(harness: &str, root: &Path, state: &NativeState) -> Result<bool> {
    let f = &state.fragment;
    Ok(match harness {
        "claude" => {
            let env = &f["env"];
            if state.kind == "oauth" {
                let c = claude_credentials(root)?;
                oauth(
                    &c["claudeAiOauth"]["accessToken"],
                    &c["claudeAiOauth"]["refreshToken"],
                    &c["claudeAiOauth"]["expiresAt"],
                ) || oauth(
                    &env["CLAUDE_CODE_OAUTH_TOKEN"],
                    &env["CLAUDE_CODE_OAUTH_REFRESH_TOKEN"],
                    &Value::Null,
                )
            } else {
                nonempty(&env["ANTHROPIC_API_KEY"])
                    || nonempty(&env["ANTHROPIC_AUTH_TOKEN"])
                    || nonempty(&f["apiKey"])
                    || nonempty(&json_doc(&root.join("settings.json"))?["apiKeyHelper"])
                    || cc_switch::CLAUDE_PROTOCOL_SELECTORS
                        .iter()
                        .any(|k| env[*k].as_str().is_some_and(|s| matches!(s, "1" | "true")))
            }
        }
        "codex" => {
            if f["model_provider"] == "remote_codex" {
                return Ok(false);
            }
            let a = codex_auth(root, state)?;
            if state.kind == "oauth" {
                codex_oauth(&a)
            } else {
                nonempty(&a["OPENAI_API_KEY"])
                    || nonempty(&f["experimental_bearer_token"])
                    || nonempty(&f["provider"]["experimental_bearer_token"])
                    || (nonempty(&f["provider"]["base_url"])
                        && f["provider"]["requires_openai_auth"] == false
                        && f["provider"].get("env_key").is_none())
                    || f["provider"]["env_key"]
                        .as_str()
                        .and_then(|k| std::env::var(k).ok())
                        .is_some_and(|v| !v.trim().is_empty())
            }
        }
        "gemini" => {
            if state.kind == "oauth" {
                let c = credentials(&root.join("oauth_creds.json"))?;
                oauth(&c["access_token"], &c["refresh_token"], &c["expiry_date"])
            } else {
                nonempty(&f["env"]["GEMINI_API_KEY"])
                    || nonempty(&f["env"]["GOOGLE_API_KEY"])
                    || (f["selectedType"] == "vertex-ai" && gemini_adc(root)?)
            }
        }
        "grok" => {
            if state.kind == "oauth" {
                grok_auth(&credentials(&root.join("auth.json"))?) == Some("oauth")
            } else {
                nonempty(&f["selectedModel"]["api_key"])
                    || grok_auth(&credentials(&root.join("auth.json"))?) == Some("config")
            }
        }
        "deepseek" => {
            let creds: Value =
                serde_yaml::from_str(&read(&root.join(".credentials.yaml"))?.unwrap_or_default())
                    .map_err(|_| anyhow!("Native DSH credentials are invalid"))?;
            f["credentialRef"]
                .as_str()
                .is_some_and(|key| nonempty(&creds["refs"][key]))
        }
        _ => false,
    })
}

fn capture(harness: &str, root: &Path) -> Result<NativeState> {
    let mut fragment = provider_config::capture(harness, root)?;
    let mut auth = None;
    let kind = match harness {
        "claude" => {
            let env = &fragment["env"];
            if nonempty(&env["ANTHROPIC_API_KEY"])
                || nonempty(&env["ANTHROPIC_AUTH_TOKEN"])
                || nonempty(&fragment["apiKey"])
                || nonempty(&json_doc(&root.join("settings.json"))?["apiKeyHelper"])
                || cc_switch::CLAUDE_PROTOCOL_SELECTORS
                    .iter()
                    .any(|k| env[*k].as_str().is_some_and(|s| matches!(s, "1" | "true")))
            {
                "config"
            } else {
                "oauth"
            }
        }
        "codex" => {
            let current = credentials(&root.join("auth.json"))?;
            auth = Some(auth_fragment(&current));
            if codex_oauth(&current) && !nonempty(&current["OPENAI_API_KEY"]) {
                "oauth"
            } else {
                "config"
            }
        }
        "gemini" => {
            let doc = json_doc(&root.join("settings.json"))?;
            fragment["selectedType"] = doc
                .pointer("/security/auth/selectedType")
                .cloned()
                .unwrap_or(Value::Null);
            fragment["model"] = doc.pointer("/model/name").cloned().unwrap_or(Value::Null);
            let c = credentials(&root.join("oauth_creds.json"))?;
            if fragment["selectedType"] == "oauth-personal"
                || (fragment["selectedType"].is_null()
                    && !nonempty(&fragment["env"]["GEMINI_API_KEY"])
                    && !nonempty(&fragment["env"]["GOOGLE_API_KEY"])
                    && oauth(&c["access_token"], &c["refresh_token"], &c["expiry_date"]))
            {
                fragment["selectedType"] = json!("oauth-personal");
                "oauth"
            } else {
                "config"
            }
        }
        "grok" => {
            let doc = toml_doc(&root.join("config.toml"))?;
            fragment = json!({"default":doc.get("models").and_then(|v|v.get("default")).and_then(|v|v.as_str())});
            // A model-local key takes precedence over the subscription account.
            let selected = fragment["default"].as_str().unwrap_or("").to_owned();
            if doc
                .get("model")
                .and_then(|v| v.get(selected.as_str()))
                .is_some()
            {
                let snapshot: toml::Value = doc
                    .to_string()
                    .parse()
                    .map_err(|_| anyhow!("Native Grok configuration is invalid"))?;
                fragment["selectedModel"] =
                    serde_json::to_value(&snapshot["model"][selected.as_str()])?;
            }
            let keyed = doc
                .get("model")
                .and_then(|v| v.get(selected.as_str()))
                .and_then(|v| v.get("api_key"))
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty());
            if !keyed && grok_auth(&credentials(&root.join("auth.json"))?) == Some("oauth") {
                "oauth"
            } else {
                "config"
            }
        }
        "deepseek" => {
            fragment = capture_dsh(root)?;
            "config"
        }
        _ => bail!("Unsupported harness"),
    };
    Ok(NativeState {
        root: root.to_owned(),
        fragment,
        auth,
        kind: kind.into(),
        generation: uuid::Uuid::new_v4().to_string(),
        selected: false,
    })
}

const DSH_FIELDS: &[(&str, &[&str])] = &[
    ("acp", &["provider", "model"]),
    ("agent-default-model", &["provider", "model"]),
    ("llm-deepseek", &["baseURL", "apiKeyEnv", "models"]),
];
fn capture_dsh(root: &Path) -> Result<Value> {
    let rows: Value = serde_yaml::from_str(
        &read(&root.join("profiles/acp/cordis.patch.yml"))?.unwrap_or("[]".into()),
    )
    .map_err(|_| anyhow!("Native DSH configuration is invalid"))?;
    let rows = rows
        .as_array()
        .ok_or_else(|| anyhow!("Native DSH patch must be an array"))?;
    let mut out = json!({});
    for (id, keys) in DSH_FIELDS {
        let row = rows.iter().rev().find(|r| r["id"] == *id);
        for key in *keys {
            out[*id][*key] = row
                .and_then(|r| r["config"].get(*key))
                .cloned()
                .unwrap_or(Value::Null);
        }
    }
    if let Ok(profile) = dsh::import_live(root, "Native") {
        let creds: Value =
            serde_yaml::from_str(&read(&root.join(".credentials.yaml"))?.unwrap_or_default())
                .map_err(|_| anyhow!("Native DSH credentials are invalid"))?;
        // Keep a reference to the in-place credential, never a second copy.
        out["credentialRef"] = creds["refs"]
            .as_object()
            .and_then(|refs| {
                refs.iter()
                    .find(|(_, v)| v.as_str() == Some(&profile.api_key))
            })
            .map(|(k, _)| json!(k))
            .unwrap_or(Value::Null);
    }
    out["llm-pi-ai"] = rows
        .iter()
        .rev()
        .find(|r| r["id"] == "llm-pi-ai")
        .and_then(|r| r.pointer("/config/providers/remote-codex"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(out)
}

/// Called before a managed activation can overwrite any native provider field.
pub(super) fn preserve(s: &mut Store, harness: &str, root: &Path) -> Result<()> {
    if s.active.contains_key(harness) {
        migrate(s, harness, root)?;
        if harness == "codex" {
            if let Some(n) = s
                .native
                .get_mut(harness)
                .filter(|n| n.root == root && n.kind == "oauth")
            {
                let current = credentials(&root.join("auth.json"))?;
                if codex_oauth(&current) {
                    n.auth = Some(codex_auth(root, n)?);
                }
            }
        }
        return Ok(());
    }
    let mut candidate = capture(harness, root)?;
    candidate.selected = s
        .native
        .get(harness)
        .is_some_and(|old| old.root == root && old.kind == candidate.kind && old.selected);
    if usable(harness, root, &candidate)? {
        s.native.insert(harness.into(), candidate);
    } else {
        s.native.remove(harness);
    }
    Ok(())
}

// Only backups explicitly taken while no managed profile was selected are
// eligible. Never treat a recent API->API snapshot as the native baseline.
fn migrate(s: &mut Store, harness: &str, root: &Path) -> Result<()> {
    if s.native.get(harness).is_some_and(|n| n.root == root) {
        return Ok(());
    }
    let Some(backup) = s
        .backups
        .iter()
        .find(|b| b.harness == harness && b.active.is_none())
    else {
        return Ok(());
    };
    // Parse historical bytes in an isolated directory; never restore an old
    // whole-file backup over current user files just to discover a fragment.
    let temp = tempfile::tempdir()?;
    for f in &backup.files {
        let Ok(relative) = f.path.strip_prefix(root) else {
            return Ok(());
        };
        if let Some(content) = &f.content {
            private_write(&temp.path().join(relative), content.as_bytes())?;
        }
    }
    let Ok(mut n) = capture(harness, temp.path()) else {
        return Ok(());
    };
    n.root = root.to_owned();
    if harness == "claude" && nonempty(&json_doc(&root.join("settings.json"))?["apiKeyHelper"]) {
        n.kind = "config".into();
    }
    if harness == "grok"
        && grok_auth(&credentials(&root.join("auth.json"))?) == Some("oauth")
        && !nonempty(&n.fragment["selectedModel"]["api_key"])
    {
        n.kind = "oauth".into();
    }
    // Claude/Gemini/Grok login files were never overwritten or backed up.
    if usable(harness, root, &n)? {
        s.native.insert(harness.into(), n);
    }
    Ok(())
}

fn status(s: &Store, harness: &str, root: &Path) -> Result<Value> {
    let Some(n) = s.native.get(harness).filter(|n| n.root == root) else {
        return Ok(json!({"available":false,"active":false,"kind":"config"}));
    };
    let available = usable(harness, root, n)?;
    let live = provider_config::capture(harness, root)?;
    let mut hash = Sha256::new();
    hash.update(n.generation.as_bytes());
    hash.update(serde_json::to_vec(&n)?);
    hash.update(serde_json::to_vec(&live)?);
    // Include the fields the generic provider fragment intentionally excludes.
    if matches!(harness, "grok" | "gemini" | "deepseek") {
        let filename = match harness {
            "grok" => "config.toml",
            "gemini" => "settings.json",
            _ => "profiles/acp/cordis.patch.yml",
        };
        hash.update(serde_json::to_vec(&read(&root.join(filename))?)?);
    }
    hash.update(serde_json::to_vec(&s.active.get(harness))?);
    if harness == "codex" {
        hash.update(serde_json::to_vec(&codex_auth(root, n)?)?);
    }
    Ok(
        json!({"available":available,"active":available && !s.active.contains_key(harness),"kind":n.kind,"revision":hex::encode(hash.finalize())}),
    )
}

/// Only this inventory is exposed to the local owner. Connection catalogs and
/// plugin connection selectors intentionally never call this API.
pub fn inventory_at(dir: &Path, roots: &BTreeMap<String, PathBuf>) -> Result<Value> {
    let mut s = load(dir)?;
    let before = serde_json::to_vec(&s)?;
    let mut out = json!({});
    for (h, root) in roots {
        if !HARNESSES.contains(&h.as_str()) {
            continue;
        }
        let result = if s.active.contains_key(h) {
            migrate(&mut s, h, root)
        } else {
            // Keep the opaque generation stable when only unrelated config changes.
            let old = s.native.get(h).cloned();
            preserve(&mut s, h, root).map(|()| {
                if let (Some(old), Some(next)) = (old, s.native.get_mut(h)) {
                    if old.root == next.root
                        && old.fragment == next.fragment
                        && old.auth == next.auth
                        && old.kind == next.kind
                    {
                        next.generation = old.generation;
                    }
                }
            })
        };
        out[h] = result
            .and_then(|()| status(&s, h, root))
            .unwrap_or_else(|_| json!({"available":false,"active":false,"kind":"config"}));
    }
    if before != serde_json::to_vec(&s)? {
        save(dir, &s)?;
    }
    Ok(out)
}

pub fn inventory(dir: &Path) -> Result<Value> {
    inventory_at(
        dir,
        &HARNESSES.into_iter().map(|h| (h.into(), home(h))).collect(),
    )
}

fn set_optional(doc: &mut Value, path: &[&str], value: &Value) -> Result<()> {
    let (key, parents) = path.split_last().unwrap();
    let mut target = doc;
    for parent in parents {
        if target.get(*parent).is_none() {
            if value.is_null() {
                return Ok(());
            }
            target[*parent] = json!({});
        }
        target = target.get_mut(*parent).unwrap();
        ensure!(
            target.is_object(),
            "Native configuration has an invalid provider object"
        );
    }
    if value.is_null() {
        target.as_object_mut().unwrap().remove(*key);
    } else {
        target[*key] = value.clone();
    }
    Ok(())
}

fn edits(s: &Store, harness: &str, root: &Path, n: &NativeState) -> Result<Vec<(PathBuf, String)>> {
    let f = &n.fragment;
    Ok(match harness {
        "claude" => {
            let path = root.join("settings.json");
            let mut doc = json_doc(&path)?;
            provider_config::project_claude(&mut doc, f)?;
            vec![(path, serde_json::to_string_pretty(&doc)?)]
        }
        "codex" => {
            let path = root.join("config.toml");
            let mut doc = toml_doc(&path)?;
            let mut fragment = f.clone();
            let provider = fragment.as_object_mut().unwrap().remove("provider");
            provider_config::project_codex(&mut doc, &fragment)?;
            if let (Some(provider), Some(selected)) = (provider, f["model_provider"].as_str()) {
                let table = toml::Value::try_from(provider)
                    .map_err(|_| anyhow!("Invalid native provider"))?;
                let mut parsed = DocumentMut::new();
                parsed["native"] = toml_edit::Item::Table(toml_edit::Table::new());
                // Reuse the typed projector's JSON-to-TOML conversion, then move
                // its slot to the original selected provider name.
                provider_config::project_codex(
                    &mut parsed,
                    &json!({"provider":serde_json::to_value(table)?}),
                )?;
                doc["model_providers"][selected] =
                    parsed["model_providers"]["remote_codex"].clone();
            }
            let auth_path = root.join("auth.json");
            let mut auth = json_doc(&auth_path)?;
            for key in ["auth_mode", "OPENAI_API_KEY", "tokens", "last_refresh"] {
                auth.as_object_mut().unwrap().remove(key);
            }
            for (key, value) in codex_auth(root, n)?.as_object().unwrap() {
                auth[key] = value.clone();
            }
            vec![
                (path, doc.to_string()),
                (auth_path, serde_json::to_string_pretty(&auth)?),
            ]
        }
        "gemini" => {
            let path = root.join("settings.json");
            let mut doc = json_doc(&path)?;
            set_optional(
                &mut doc,
                &["security", "auth", "selectedType"],
                &f["selectedType"],
            )?;
            set_optional(&mut doc, &["model", "name"], &f["model"])?;
            let env = root.join(".env");
            let text = provider_config::project_gemini_env(&read(&env)?.unwrap_or_default(), f)?;
            vec![(path, serde_json::to_string_pretty(&doc)?), (env, text)]
        }
        "grok" => {
            let path = root.join("config.toml");
            let mut doc = toml_doc(&path)?;
            // Only the provider fields managed by configure_grok_models belong
            // to us. A user may have added other fields to these tables since.
            for (id, original) in &s.grok_models {
                let original = original
                    .as_ref()
                    .map(|text| {
                        text.parse::<DocumentMut>()
                            .map_err(|_| anyhow!("Invalid native Grok model"))
                    })
                    .transpose()?;
                for key in [
                    "name",
                    "model",
                    "base_url",
                    "api_key",
                    "api_backend",
                    "auth_provider",
                    "env_key",
                    "model_provider",
                    "api_base_url",
                    "extra_headers",
                    "env_http_headers",
                    "query_params",
                    "mtls_cert_dir",
                ] {
                    if let Some(v) = original
                        .as_ref()
                        .and_then(|d| d.get("model"))
                        .and_then(|m| m.get(id))
                        .and_then(|m| m.get(key))
                    {
                        doc["model"][id][key] = v.clone();
                    } else if let Some(table) = doc
                        .get_mut("model")
                        .and_then(|v| v.get_mut(id))
                        .and_then(|v| v.as_table_like_mut())
                    {
                        table.remove(key);
                    }
                }
            }
            // The synthetic aliases were wholly generated by the managed slot.
            if let Some(table) = doc.get_mut("model").and_then(|v| v.as_table_like_mut()) {
                for id in s.grok_models.keys().map(|id| format!("remote-codex/{id}")) {
                    table.remove(&id);
                }
            }
            if let Some(default) = f["default"].as_str() {
                doc["models"]["default"] = value(default);
            } else if let Some(models) = doc.get_mut("models").and_then(|v| v.as_table_like_mut()) {
                models.remove("default");
            }
            vec![(path, doc.to_string())]
        }
        "deepseek" => {
            let path = root.join("profiles/acp/cordis.patch.yml");
            let mut rows: Value = serde_yaml::from_str(&read(&path)?.unwrap_or("[]".into()))
                .map_err(|_| anyhow!("Invalid DSH patch"))?;
            let rows = rows
                .as_array_mut()
                .ok_or_else(|| anyhow!("Invalid DSH patch"))?;
            for (id, keys) in DSH_FIELDS {
                if let Some(row) = rows
                    .iter_mut()
                    .rev()
                    .find(|r| r["id"] == *id && r.get("insert").is_none())
                {
                    for key in *keys {
                        set_optional(row, &["config", key], &f[*id][*key])?;
                    }
                } else if f[*id]
                    .as_object()
                    .is_some_and(|m| m.values().any(|v| !v.is_null()))
                {
                    let mut row = json!({"id":id,"config":{}});
                    for key in *keys {
                        set_optional(&mut row, &["config", key], &f[*id][*key])?;
                    }
                    rows.push(row);
                }
            }
            if let Some(row) = rows
                .iter_mut()
                .rev()
                .find(|r| r["id"] == "llm-pi-ai" && r.get("insert").is_none())
            {
                set_optional(
                    row,
                    &["config", "providers", "remote-codex"],
                    &f["llm-pi-ai"],
                )?;
            }
            vec![(path, serde_yaml::to_string(rows)?)]
        }
        _ => bail!("Unsupported harness"),
    })
}

pub fn check_at(dir: &Path, harness: &str, root: &Path, revision: Option<&str>) -> Result<()> {
    ensure!(HARNESSES.contains(&harness), "Unsupported harness");
    let list = inventory_at(dir, &BTreeMap::from([(harness.into(), root.to_owned())]))?;
    ensure!(
        list[harness]["available"] == true,
        "No usable native configuration is available for this harness"
    );
    if let Some(expected) = revision {
        ensure!(
            list[harness]["revision"].as_str() == Some(expected),
            "Native edit conflict: reload before applying"
        );
    }
    Ok(())
}

pub fn activate_at(
    dir: &Path,
    harness: &str,
    root: &Path,
    revision: Option<&str>,
) -> Result<Value> {
    activate_with(dir, harness, root, revision, |_| Ok(()))
}

// Hook is private and used only to prove rollback against an external writer.
fn activate_with(
    dir: &Path,
    harness: &str,
    root: &Path,
    revision: Option<&str>,
    mut after_commit: impl FnMut(usize) -> Result<()>,
) -> Result<Value> {
    check_at(dir, harness, root, revision)?;
    let mut s = load(dir)?;
    let n = s
        .native
        .get(harness)
        .cloned()
        .ok_or_else(|| anyhow!("Native configuration unavailable"))?;
    let mut paths = match harness {
        "codex" => vec![root.join("config.toml"), root.join("auth.json")],
        "gemini" => vec![root.join("settings.json"), root.join(".env")],
        "claude" => vec![root.join("settings.json")],
        "grok" => vec![root.join("config.toml")],
        "deepseek" => vec![root.join("profiles/acp/cordis.patch.yml")],
        _ => bail!("Unsupported harness"),
    };
    paths.extend([dir.join("profiles.json"), dir.join("connections.json")]);
    let before = paths
        .iter()
        .map(|p| Ok((p.clone(), read(p)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if let Some(expected) = revision {
        ensure!(
            status(&s, harness, root)?["revision"].as_str() == Some(expected),
            "Native edit conflict: reload before applying"
        );
    }
    let mut planned = edits(&s, harness, root, &n)?;
    if let Some(id) = s.active.get(harness) {
        if let Some(old) = s.profiles.iter_mut().find(|p| &p.id == id) {
            old.settings_config = provider_config::capture(harness, root)?;
            if let Ok(live) = read_live_profile(harness, &old.name, root) {
                old.base_url = live.base_url;
                old.api_key = live.api_key;
                old.model = live.model;
                old.auth_type = live.auth_type;
                old.api_type = live.api_type;
                old.context_window = live.context_window;
            }
        }
    }
    if harness == "codex" {
        s.native.get_mut(harness).unwrap().auth = Some(codex_auth(root, &n)?);
    }
    s.native.get_mut(harness).unwrap().selected = true;
    if harness == "grok" {
        s.grok_models.clear();
    }
    s.active.remove(harness);
    planned.push((dir.join("profiles.json"), serde_json::to_string_pretty(&s)?));
    planned.push((
        dir.join("connections.json"),
        connections::without_binding(dir, harness)?,
    ));
    for (path, expected) in &before {
        ensure!(read(path)? == *expected, "{}", write_engine::CONFLICT);
    }
    let staged = planned
        .iter()
        .map(|(p, text)| write_engine::stage(p, text.as_bytes()))
        .collect::<Result<Vec<_>>>()?;
    let mut committed = Vec::new();
    let result = (|| -> Result<()> {
        for (i, ((path, text), file)) in planned.iter().zip(staged).enumerate() {
            write_engine::commit(file, path, before[path].as_deref())?;
            committed.push((path, text));
            after_commit(i)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        for (path, written) in committed.into_iter().rev() {
            if read(path)?.as_deref() == Some(written.as_str()) {
                if let Some(original) = &before[path] {
                    private_write(path, original.as_bytes())?;
                } else {
                    std::fs::remove_file(path)?;
                }
            }
        }
        return Err(error);
    }
    Ok(json!({"ok":true,"native":true,"harness":harness,"restartRequired":false}))
}

// An internal launch directive, consumed before exec and never inherited by the
// child. It carries only a harness ID, never credentials.
pub(crate) const CLEAN_OAUTH_ENV: &str = "POCKYMOE_NATIVE_OAUTH_ENV";
pub(crate) fn oauth_selected(dir: &Path, harness: &str) -> bool {
    oauth_selected_at(dir, harness, &home(harness))
}
fn oauth_selected_at(dir: &Path, harness: &str, root: &Path) -> bool {
    load(dir).ok().is_some_and(|s| {
        !s.active.contains_key(harness)
            && s.native
                .get(harness)
                .is_some_and(|n| n.selected && n.kind == "oauth" && n.root == root)
    })
}
pub(crate) fn provider_env(harness: &str, key: &str) -> bool {
    match harness {
        "claude" => cc_switch::claude_provider_env(key),
        "gemini" => cc_switch::gemini_provider_env(key),
        "codex" => key.starts_with("OPENAI_") || key == "CODEX_API_KEY",
        "grok" => key.starts_with("XAI_") || key.starts_with("GROK_API_"),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
