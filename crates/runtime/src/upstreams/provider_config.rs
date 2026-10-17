//! Provider fragments and live backfill, adapted from CC Switch live/project
//! and services/provider/live.rs. Copyright (c) 2025 Jason Young, MIT.
//! See THIRD_PARTY_NOTICES.md. Global tool variables never enter a projection.
use super::*;
use sha2::{Digest, Sha256};
const MASK: &str = "[stored privately]";

pub fn revision(p: &Profile) -> String {
    let mut p = p.clone();
    p.revision = None;
    p.live_revision = None;
    hex::encode(Sha256::digest(serde_json::to_vec(&p).unwrap()))
}
fn secret(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("key")
        || k.contains("token")
        || k.contains("secret")
        || k.contains("password")
        || k.contains("headers")
        || k == "authorization"
}
pub fn redact(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        if secret(k) && !v.is_null() {
                            json!(MASK)
                        } else {
                            redact(v)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(rows) => Value::Array(rows.iter().map(redact).collect()),
        _ => v.clone(),
    }
}
pub fn restore_redacted(v: &mut Value, old: &Value) {
    if v.as_str() == Some(MASK) {
        *v = old.clone();
    } else if let Some(map) = v.as_object_mut() {
        for (key, value) in map {
            restore_redacted(value, &old[key]);
        }
    } else if let Some(rows) = v.as_array_mut() {
        for (index, row) in rows.iter_mut().enumerate() {
            restore_redacted(row, &old[index]);
        }
    }
}
pub fn remove_redacted(v: &mut Value) {
    if let Some(map) = v.as_object_mut() {
        map.retain(|_, v| v.as_str() != Some(MASK));
        for v in map.values_mut() {
            remove_redacted(v);
        }
    } else if let Some(rows) = v.as_array_mut() {
        rows.retain(|row| row.as_str() != Some(MASK));
        for row in rows {
            remove_redacted(row);
        }
    }
}
pub fn merge_defaults(value: &mut Value, defaults: Value) {
    if let (Some(value), Value::Object(defaults)) = (value.as_object_mut(), defaults) {
        for (key, default) in defaults {
            if let Some(existing) = value.get_mut(&key) {
                merge_defaults(existing, default);
            } else {
                value.insert(key, default);
            }
        }
    }
}
pub fn validate_config(p: &Profile) -> Result<()> {
    let map = p
        .settings_config
        .as_object()
        .ok_or_else(|| anyhow!("Provider configuration must be a JSON object"))?;
    if serde_json::to_vec(&p.settings_config)?.len() > 256_000 {
        bail!("Provider configuration exceeds 256 KB");
    }
    for (key, value) in map {
        let valid = match p.harness.as_str() {
            "claude" => cc_switch::claude_provider_top(key) || key == "env",
            "codex" => {
                cc_switch::codex_provider_top(key)
                    || ["provider", "agents", "memories"].contains(&key.as_str())
            }
            "gemini" => key == "env",
            "grok" => false,
            "deepseek" => ["profile", "headers"].contains(&key.as_str()),
            _ => false,
        };
        if !valid {
            bail!("Configuration contains a non-provider field: {key}");
        }
        if key == "provider" || key == "headers" {
            if !value.is_object() {
                bail!("Provider fields and headers must be objects");
            }
            if key == "headers"
                && value
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|value| !value.is_string())
            {
                bail!("Header values must be strings");
            }
        }
        if ["agents", "memories"].contains(&key.as_str()) {
            let nested = value
                .as_object()
                .ok_or_else(|| anyhow!("Nested provider fields must be an object"))?;
            if nested.keys().any(|nested| {
                !cc_switch::CODEX_FLOOR_NESTED
                    .iter()
                    .any(|path| path[0] == key && path[1] == nested)
            }) {
                bail!("Unsupported nested provider field");
            }
        }
        if key == "profile" && value.as_str() != Some("acp") {
            bail!("The DSH adapter manages the ACP profile only");
        }
        if key == "env" {
            let env = value
                .as_object()
                .ok_or_else(|| anyhow!("Provider env must be an object"))?;
            for (key, value) in env {
                let valid = match p.harness.as_str() {
                    "claude" => cc_switch::claude_provider_env(key),
                    "gemini" => cc_switch::gemini_provider_env(key),
                    _ => false,
                };
                if !valid || !value.is_string() {
                    bail!("Provider env contains an unsupported field: {key}");
                }
            }
        }
    }
    Ok(())
}
pub fn capture(harness: &str, root: &Path) -> Result<Value> {
    match harness {
        "claude" => {
            let doc = json_doc(&root.join("settings.json"))?;
            let mut fragment = json!({});
            for (key, value) in doc.as_object().unwrap() {
                if cc_switch::claude_provider_top(key) {
                    fragment[key] = value.clone();
                }
            }
            let mut env = json!({});
            if let Some(values) = doc.get("env") {
                let values = values
                    .as_object()
                    .ok_or_else(|| anyhow!("Invalid Claude env object"))?;
                for (key, value) in values {
                    if cc_switch::claude_provider_env(key) {
                        env[key] = value.clone();
                    }
                }
            }
            fragment["env"] = env;
            Ok(fragment)
        }
        "codex" => {
            let doc = toml_doc(&root.join("config.toml"))?;
            let plain: toml::Value = doc.to_string().parse()?;
            let mut fragment = json!({});
            for (key, value) in plain.as_table().unwrap() {
                if cc_switch::codex_provider_top(key) {
                    fragment[key] = serde_json::to_value(value)?;
                }
            }
            for path in cc_switch::CODEX_FLOOR_NESTED {
                if let Some(value) = plain.get(path[0]).and_then(|v| v.get(path[1])) {
                    if fragment.get(path[0]).is_none() {
                        fragment[path[0]] = json!({});
                    }
                    fragment[path[0]][path[1]] = serde_json::to_value(value)?;
                }
            }
            let selected = plain
                .get("model_provider")
                .and_then(toml::Value::as_str)
                .unwrap_or("remote_codex");
            if let Some(provider) = plain.get("model_providers").and_then(|v| v.get(selected)) {
                fragment["provider"] = serde_json::to_value(provider)?;
            }
            Ok(fragment)
        }
        "gemini" => {
            let mut env = json!({});
            for (key, value) in dotenv(&read(&root.join(".env"))?.unwrap_or_default()) {
                if cc_switch::gemini_provider_env(&key) {
                    env[&key] = json!(value);
                }
            }
            Ok(json!({"env": env}))
        }
        "deepseek" => dsh::capture(root),
        _ => Ok(json!({})),
    }
}
pub fn project_claude(doc: &mut Value, fragment: &Value) -> Result<()> {
    doc.as_object_mut()
        .unwrap()
        .retain(|key, _| !cc_switch::claude_provider_top(key));
    if let Some(env) = doc.get_mut("env") {
        env.as_object_mut()
            .ok_or_else(|| anyhow!("Invalid Claude env object"))?
            .retain(|key, _| !cc_switch::claude_provider_env(key));
    }
    for (key, value) in fragment.as_object().unwrap() {
        if key == "env" {
            for (key, value) in value.as_object().unwrap() {
                put_json(doc, "env", key, value.clone())?;
            }
        } else {
            doc[key] = value.clone();
        }
    }
    Ok(())
}
pub fn project_codex(doc: &mut DocumentMut, fragment: &Value) -> Result<()> {
    for key in ["agents", "memories"] {
        if doc.get(key).is_some_and(|value| !value.is_table_like()) {
            bail!("Existing Codex nested provider fields must be tables");
        }
    }
    for key in cc_switch::CODEX_FLOOR_TOP
        .iter()
        .chain(cc_switch::CODEX_EXCLUSIVE_TOP)
    {
        doc.remove(key);
    }
    for path in cc_switch::CODEX_FLOOR_NESTED {
        if let Some(table) = doc.get_mut(path[0]).and_then(|v| v.as_table_like_mut()) {
            table.remove(path[1]);
        }
    }
    // Provider tables unrelated to the managed slot remain user-owned.
    if let Some(table) = doc
        .get_mut("model_providers")
        .and_then(|v| v.as_table_like_mut())
    {
        table.remove("remote_codex");
    }
    let mut root = toml::map::Map::new();
    for (key, value) in fragment.as_object().unwrap() {
        if ["provider", "agents", "memories"].contains(&key.as_str()) {
            continue;
        }
        root.insert(
            key.clone(),
            serde_json::from_value::<toml::Value>(value.clone())?,
        );
    }
    let extra: DocumentMut = toml::to_string(&root)?.parse()?;
    for (key, item) in extra.iter() {
        doc[key] = item.clone();
    }
    for path in cc_switch::CODEX_FLOOR_NESTED {
        if let Some(value) = fragment.get(path[0]).and_then(|v| v.get(path[1])) {
            let value: toml::Value = serde_json::from_value(value.clone())?;
            let mut single = toml::map::Map::new();
            single.insert(path[1].into(), value);
            let extra: DocumentMut = toml::to_string(&single)?.parse()?;
            doc[path[0]][path[1]] = extra[path[1]].clone();
        }
    }
    if let Some(provider) = fragment.get("provider") {
        let value: toml::Value = serde_json::from_value(provider.clone())?;
        let extra: DocumentMut = toml::to_string(&value)?.parse()?;
        doc["model_providers"]["remote_codex"] = toml_edit::Item::Table(extra.as_table().clone());
    }
    Ok(())
}
pub(super) fn dotenv(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim();
            Some((
                key.trim().into(),
                serde_json::from_str::<String>(value)
                    .unwrap_or_else(|_| value.trim_matches('\'').into()),
            ))
        })
        .collect()
}
pub fn project_gemini_env(text: &str, fragment: &Value) -> Result<String> {
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            line.split_once('=')
                .is_none_or(|(key, _)| !cc_switch::gemini_provider_env(key.trim()))
        })
        .map(str::to_owned)
        .collect();
    if let Some(env) = fragment.get("env").and_then(Value::as_object) {
        for (key, value) in env {
            lines.push(format!("{key}={}", serde_json::to_string(value)?));
        }
    }
    Ok(lines.join("\n") + "\n")
}

pub fn strip_basic_auth(fragment: &mut Value) {
    for key in [
        "experimental_bearer_token",
        "openai_base_url",
        "model",
        "model_provider",
        "base_url",
        "wire_api",
    ] {
        fragment.as_object_mut().unwrap().remove(key);
    }
    if let Some(env) = fragment.get_mut("env").and_then(Value::as_object_mut) {
        for key in [
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
            "CLAUDE_CODE_OAUTH_SCOPES",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ] {
            env.remove(key);
        }
    }
    if let Some(provider) = fragment.get_mut("provider").and_then(Value::as_object_mut) {
        for key in [
            "experimental_bearer_token",
            "env_key",
            "requires_openai_auth",
            "base_url",
        ] {
            provider.remove(key);
        }
    }
}
pub fn headers(p: &Profile) -> Vec<(String, String)> {
    let object = p
        .settings_config
        .pointer("/provider/http_headers")
        .or_else(|| p.settings_config.get("headers"));
    let mut headers = object
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| value.as_str().map(|v| (name.clone(), v.into())))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(text) = p
        .settings_config
        .pointer("/env/ANTHROPIC_CUSTOM_HEADERS")
        .or_else(|| p.settings_config.pointer("/env/GEMINI_CLI_CUSTOM_HEADERS"))
        .and_then(Value::as_str)
    {
        headers.extend(text.lines().filter_map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.trim().into(), value.trim().into()))
        }));
    }
    headers
}
