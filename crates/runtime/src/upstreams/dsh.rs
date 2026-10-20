//! DSH upstream projection. Only the ACP profile's provider/default-model
//! overrides and a dedicated credential reference are changed.
use super::*;
use serde_yaml::Value as Yaml;
const KEY_REF: &str = "REMOTE_CODEX_DSH_API_KEY"; // Legacy managed reference remains importable.
fn credential_ref(p: &Profile) -> String {
    format!(
        "REMOTE_CODEX_DSH_{}",
        p.id.replace('-', "_").to_ascii_uppercase()
    )
}
fn profile_name(p: &Profile) -> Result<&str> {
    let name = p
        .settings_config
        .get("profile")
        .and_then(Value::as_str)
        .unwrap_or("acp");
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!("Invalid DSH profile name");
    }
    Ok(name)
}
pub fn paths(p: &Profile, root: &Path) -> Result<Vec<PathBuf>> {
    Ok(vec![
        root.join(".credentials.yaml"),
        root.join("profiles")
            .join(profile_name(p)?)
            .join("cordis.patch.yml"),
    ])
}
fn yaml_doc(path: &Path, default: &str) -> Result<Yaml> {
    serde_yaml::from_str(&read(path)?.unwrap_or(default.into()))
        .map_err(|_| anyhow!("Existing DSH YAML is invalid; repair it before switching upstreams"))
}
/// The model list for an entry: the selected model first, then the models
/// written before. DSH threads keep their `["provider","model"]` value, so a
/// switched model must stay configured or those threads can no longer resume.
fn models_with(rows: &[Yaml], id: &str, path: &[&str], model: Value) -> Result<Value> {
    let selected = model["id"].clone();
    let mut models = vec![model];
    for entry in previous_models(rows, id, path)? {
        if entry["id"] != selected {
            models.push(entry);
        }
    }
    Ok(Value::Array(models))
}

/// Model entries an earlier switch wrote for this entry.
fn previous_models(rows: &[Yaml], id: &str, path: &[&str]) -> Result<Vec<Value>> {
    let existing = rows
        .iter()
        .rev()
        .find(|row| row.get("id").and_then(Yaml::as_str) == Some(id) && row.get("insert").is_none())
        .and_then(|row| {
            path.iter()
                .try_fold(row.get("config")?, |value, key| value.get(*key))
        })
        .and_then(Yaml::as_sequence);
    let mut models = Vec::new();
    for entry in existing.into_iter().flatten() {
        let entry: Value = serde_json::to_value(entry)?;
        if entry["id"].is_string() {
            models.push(entry);
        }
    }
    Ok(models)
}

/// DSH's reasoning levels; Codex-only levels (`ultra`) have no DSH equivalent.
const DSH_EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// DSH's reasoning levels as pi-ai routes declare them, keyed by DSH level
/// with the upstream's own spelling as the wire value. DSH calls "no
/// reasoning" `off`; Codex-only levels (`ultra`) have no DSH equivalent.
fn dsh_efforts<'a>(efforts: impl Iterator<Item = &'a str>) -> serde_json::Map<String, Value> {
    efforts
        .filter_map(|effort| match effort {
            "none" | "off" => Some(("off", effort)),
            effort if DSH_EFFORTS.contains(&effort) => Some((effort, effort)),
            _ => None,
        })
        .map(|(level, wire)| (level.to_string(), json!(wire)))
        .collect()
}

fn dsh_level(effort: &str) -> Option<&str> {
    match effort {
        "none" | "off" => Some("off"),
        effort => DSH_EFFORTS.contains(&effort).then_some(effort),
    }
}

/// At most this many models are declared on the route; the selected and
/// earlier ones always are.
const MAX_MODELS: usize = 200;

/// One OpenAI-compatible model as DSH's pi-ai provider declares it. Each fact
/// comes from what the upstream declared, then Codex's catalog, then what an
/// earlier write had. The output cap is left to the route (see `changes`):
/// a declared maximum would become every request's `max_tokens`.
fn compatible_model(
    id: &str,
    configured_window: Option<i64>,
    declared: Option<&DiscoveredModel>,
    meta: Option<&super::model_meta::ModelMeta>,
    previous: Option<&Value>,
) -> Value {
    let window = configured_window
        .or_else(|| declared.and_then(|m| m.context_window))
        .or_else(|| meta.and_then(|meta| meta.context_window))
        .or_else(|| previous.and_then(|entry| entry["contextWindow"].as_i64()));
    let name = declared
        .map(|m| m.name.trim())
        .filter(|name| !name.is_empty())
        .unwrap_or(id);
    let mut model = json!({"id": id, "name": name});
    if let Some(window) = window {
        model["contextWindow"] = json!(window);
    }
    match declared.and_then(|m| m.reasoning.as_ref()) {
        // The upstream says this model does not reason.
        Some(cap) if cap.efforts.is_empty() => model["reasoningEfforts"] = json!(false),
        Some(cap) => {
            let efforts = dsh_efforts(cap.efforts.iter().map(|e| e.reasoning_effort.as_str()));
            if !efforts.is_empty() {
                model["reasoningEfforts"] = Value::Object(efforts);
            }
        }
        None => {}
    }
    if model.get("reasoningEfforts").is_none() {
        let efforts = meta
            .map(|meta| dsh_efforts(meta.efforts.iter().map(String::as_str)))
            .unwrap_or_default();
        if !efforts.is_empty() {
            model["reasoningEfforts"] = Value::Object(efforts);
        } else if let Some(previous) = previous
            .map(|entry| &entry["reasoningEfforts"])
            .filter(|v| !v.is_null())
        {
            model["reasoningEfforts"] = previous.clone();
        }
    }
    let input = declared
        .map(|m| &m.input)
        .filter(|input| !input.is_empty())
        .or_else(|| {
            meta.map(|meta| &meta.input)
                .filter(|input| !input.is_empty())
        });
    match input {
        Some(input) => model["input"] = json!(input),
        None => {
            if let Some(input) = previous
                .map(|entry| &entry["input"])
                .filter(|v| v.is_array())
            {
                model["input"] = input.clone();
            }
        }
    }
    model
}

/// Gateways also fail mid-stream with errors DSH does not classify (for
/// example `upstream_http2_stream_error`). Retry those with backoff before
/// the turn fails, as DSH already does for its known transient codes.
fn retry_policy() -> Value {
    json!({
        "mode": "normal",
        "maxRetries": 6,
        "retryableCodes": ["EMPTY_RESPONSE", "RATE_LIMIT", "SERVER", "TIMEOUT", "TRANSPORT", "PI_AI_ERROR"],
        "backoff": {"initialDelayMs": 1000, "maxDelayMs": 20000, "jitterRatio": 0.2},
    })
}

fn set_entry(rows: &mut Vec<Yaml>, id: &str, config: Value) -> Result<()> {
    let row = rows.iter_mut().rev().find(|row| {
        row.get("id").and_then(Yaml::as_str) == Some(id) && row.get("insert").is_none()
    });
    if let Some(row) = row {
        let map = row
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("Invalid DSH patch entry"))?;
        let key = Yaml::String("config".into());
        let current = map.entry(key).or_insert(Yaml::Mapping(Default::default()));
        let current = current
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("Invalid DSH provider configuration"))?;
        let incoming: Yaml = serde_yaml::to_value(config)?;
        for (key, value) in incoming.as_mapping().unwrap() {
            // Preserve other providers in the same pi-ai entry.
            if key.as_str() == Some("providers") {
                let providers = current
                    .entry(key.clone())
                    .or_insert(Yaml::Mapping(Default::default()));
                let providers = providers
                    .as_mapping_mut()
                    .ok_or_else(|| anyhow!("Invalid DSH provider map"))?;
                for (key, value) in value.as_mapping().unwrap() {
                    providers.insert(key.clone(), value.clone());
                }
            } else {
                current.insert(key.clone(), value.clone());
            }
        }
    } else {
        rows.push(serde_yaml::to_value(json!({"id":id,"config":config}))?);
    }
    Ok(())
}
pub fn changes(p: &Profile, root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let paths = paths(p, root)?;
    let key_ref = credential_ref(p);
    let mut credentials = yaml_doc(&paths[0], "version: 1\nrefs: {}\n")?;
    let map = credentials
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("Invalid DSH credentials document"))?;
    map.entry(Yaml::String("version".into()))
        .or_insert(Yaml::Number(1.into()));
    let refs = map
        .entry(Yaml::String("refs".into()))
        .or_insert(Yaml::Mapping(Default::default()));
    refs.as_mapping_mut()
        .ok_or_else(|| anyhow!("Invalid DSH credential references"))?
        .insert(
            Yaml::String(key_ref.clone()),
            Yaml::String(p.api_key.clone()),
        );
    let mut patch = yaml_doc(&paths[1], "[]\n")?;
    let rows = patch
        .as_sequence_mut()
        .ok_or_else(|| anyhow!("DSH profile patch must be a YAML sequence"))?;
    let provider = if p.api_type == "anthropic" {
        let models = models_with(
            rows,
            "llm-deepseek",
            &["models"],
            json!({"id":p.model,"contextWindow":p.context_window}),
        )?;
        set_entry(
            rows,
            "llm-deepseek",
            json!({"baseURL":p.base_url,"apiKeyEnv":key_ref,"models":models}),
        )?;
        "deepseek-official"
    } else {
        // The form's untouched default window is not a fact about the model.
        let configured = (p.context_window != super::context_window()).then_some(p.context_window);
        let declared = |id: &str| {
            p.models
                .iter()
                .find(|m| m.id == id || m.aliases.iter().any(|a| a == id))
        };
        let meta = super::model_meta::lookup(&p.model);
        let selected = declared(&p.model);
        let mut models = vec![compatible_model(
            &p.model,
            configured,
            selected,
            meta.as_ref(),
            None,
        )];
        for previous in
            previous_models(rows, "llm-pi-ai", &["providers", "remote-codex", "models"])?
        {
            let Some(id) = previous["id"].as_str().filter(|id| *id != p.model) else {
                continue;
            };
            let meta = super::model_meta::lookup(id);
            models.push(compatible_model(
                id,
                None,
                declared(id),
                meta.as_ref(),
                Some(&previous),
            ));
        }
        // Everything else the upstream serves, so a thread can switch models.
        for model in &p.models {
            if models.len() >= MAX_MODELS {
                break;
            }
            if models.iter().all(|entry| entry["id"] != model.id.as_str()) {
                let meta = super::model_meta::lookup(&model.id);
                models.push(compatible_model(
                    &model.id,
                    None,
                    Some(model),
                    meta.as_ref(),
                    None,
                ));
            }
        }
        if models[0].get("contextWindow").is_none() {
            models[0]["contextWindow"] = json!(p.context_window);
        }
        let mut route = json!({
            "displayName":p.name, "baseURL":p.base_url,"apiKeyEnv":key_ref,
            "api":if p.api_type == "responses" {"openai-responses"} else {"openai-completions"},
            "models":models,
            "headers":p.settings_config.get("headers").cloned().unwrap_or(json!({})),
            "retryPolicy": retry_policy(),
        });
        // DSH would send its 32k default as `max_output_tokens`. Like Codex,
        // send none on the Responses API so the model's own limit applies.
        if p.api_type == "responses" {
            route["compat"] = json!({"supportsMaxOutputTokens": false});
        }
        // The model's own default level, used until a thread picks one.
        if let Some(level) = selected
            .and_then(|m| m.reasoning.as_ref())
            .and_then(|cap| cap.default.as_deref())
            .or(meta
                .as_ref()
                .and_then(|meta| meta.default_effort.as_deref()))
            .and_then(dsh_level)
        {
            route["reasoning"] = json!(level);
        }
        set_entry(
            rows,
            "llm-pi-ai",
            json!({"providers":{"remote-codex": route}}),
        )?;
        "remote-codex"
    };
    for id in ["acp", "agent-default-model"] {
        set_entry(rows, id, json!({"provider":provider,"model":p.model}))?;
    }
    Ok(vec![
        (paths[0].clone(), serde_yaml::to_string(&credentials)?),
        (paths[1].clone(), serde_yaml::to_string(&patch)?),
    ])
}
pub fn capture(root: &Path) -> Result<Value> {
    let patch = yaml_doc(&root.join("profiles/acp/cordis.patch.yml"), "[]\n")?;
    let rows = patch
        .as_sequence()
        .ok_or_else(|| anyhow!("DSH profile patch must be a sequence"))?;
    let config = |id: &str| {
        rows.iter()
            .rev()
            .find(|row| row.get("id").and_then(Yaml::as_str) == Some(id))
            .and_then(|row| row.get("config"))
    };
    let selected = config("acp")
        .and_then(|v| v.get("provider"))
        .and_then(Yaml::as_str)
        .unwrap_or("deepseek-official");
    let mut fragment = json!({"profile":"acp"});
    if selected != "deepseek-official" {
        if let Some(headers) = config("llm-pi-ai")
            .and_then(|v| v.get("providers"))
            .and_then(|v| v.get(selected))
            .and_then(|v| v.get("headers"))
        {
            fragment["headers"] = serde_json::to_value(headers)?;
        }
    }
    Ok(fragment)
}

pub fn import_live(root: &Path, name: &str) -> Result<Profile> {
    let patch = yaml_doc(&root.join("profiles/acp/cordis.patch.yml"), "[]\n")?;
    let rows = patch
        .as_sequence()
        .ok_or_else(|| anyhow!("DSH profile patch must be a sequence"))?;
    let config = |id: &str| {
        rows.iter()
            .rev()
            .find(|row| row.get("id").and_then(Yaml::as_str) == Some(id))
            .and_then(|row| row.get("config"))
    };
    let selected = config("acp")
        .and_then(|v| v.get("provider"))
        .and_then(Yaml::as_str)
        .unwrap_or("deepseek-official");
    let official = selected == "deepseek-official";
    let endpoint = if official {
        config("llm-deepseek")
    } else {
        config("llm-pi-ai")
            .and_then(|v| v.get("providers"))
            .and_then(|v| v.get(selected))
    };
    if !official && endpoint.is_none() {
        bail!("The current DSH provider is not an OpenAI compatible pi-ai route");
    }
    if !official
        && !matches!(
            endpoint.and_then(|v| v.get("api")).and_then(Yaml::as_str),
            Some("openai-responses" | "openai-completions")
        )
    {
        bail!("Import supports DSH official and explicit OpenAI-compatible routes");
    }
    if !official
        && endpoint
            .and_then(|v| v.get("baseURL"))
            .and_then(Yaml::as_str)
            .is_none()
    {
        bail!("The DSH provider needs an explicit base URL for import");
    }
    let model = config("acp")
        .and_then(|v| v.get("model"))
        .and_then(Yaml::as_str)
        .unwrap_or("deepseek-v4-flash");
    let window = endpoint
        .and_then(|v| v.get("models"))
        .and_then(Yaml::as_sequence)
        .and_then(|models| {
            models
                .iter()
                .find(|v| v.get("id").and_then(Yaml::as_str) == Some(model))
        })
        .and_then(|v| v.get("contextWindow"))
        .and_then(Yaml::as_i64)
        .unwrap_or(context_window());
    let credentials = yaml_doc(&root.join(".credentials.yaml"), "version: 1\nrefs: {}\n")?;
    let key_ref = endpoint
        .and_then(|v| v.get("apiKeyEnv"))
        .and_then(Yaml::as_str)
        .unwrap_or(if official {
            "DEEPSEEK_API_KEY"
        } else {
            KEY_REF
        });
    let key = credentials
        .get("refs")
        .and_then(|v| v.get(key_ref))
        .and_then(Yaml::as_str)
        .ok_or_else(|| anyhow!("No API key is stored in the DSH credentials file"))?;
    Ok(Profile {
        id: String::new(),
        name: name.into(),
        harness: "deepseek".into(),
        base_url: endpoint
            .and_then(|v| v.get("baseURL"))
            .and_then(Yaml::as_str)
            .unwrap_or("https://api.deepseek.com/anthropic")
            .into(),
        api_key: key.into(),
        auth_type: api_key_auth(),
        model: model.into(),
        api_type: if official {
            "anthropic"
        } else if endpoint.and_then(|v| v.get("api")).and_then(Yaml::as_str)
            == Some("openai-responses")
        {
            "responses"
        } else {
            "chat_completions"
        }
        .into(),
        context_window: window,
        settings_config: capture(root)?,
        sort_index: 0,
        revision: None,
        live_revision: None,
        models: Vec::new(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dsh_home_environment_is_isolated_and_live_import_tracks_the_selected_protocol() {
        const FLAG: &str = "POCKYMOE_UPSTREAM_DSH_TEST_CHILD";
        if std::env::var_os(FLAG).is_some() {
            let root = PathBuf::from(std::env::var_os("DSH_HOME").unwrap());
            assert_eq!(home("deepseek"), root);
            let store = root.join("store");
            let mut p = super::super::tests::fixture("deepseek");
            p.api_type = "responses".into();
            p.settings_config = json!({"profile":"acp","headers":{"X-Test":"private-header"}});
            upsert(&store, p.clone()).unwrap();
            activate(&store, &p.id).unwrap();
            let imported = import_live(&root, "Live DSH").unwrap();
            assert_eq!(imported.api_type, "responses");
            assert_eq!(imported.api_key, p.api_key);
            assert_eq!(
                capture(&root).unwrap()["headers"]["X-Test"],
                "private-header"
            );
            let mut editor = super::super::inspect_profile(&store, &p.id).unwrap();
            editor.as_object_mut().unwrap().remove("hasApiKey");
            let mut editor: Profile = serde_json::from_value(editor).unwrap();
            editor.name = "Edited DSH".into();
            save_and_apply(&store, editor).unwrap();
            assert_eq!(
                capture(&root).unwrap()["headers"]["X-Test"],
                "private-header"
            );
            let path = root.join("profiles/acp/cordis.patch.yml");
            let mut doc = yaml_doc(&path, "").unwrap();
            let rows = doc.as_sequence_mut().unwrap();
            for row in rows.iter_mut() {
                if row["id"].as_str() == Some("llm-pi-ai") {
                    let providers = row["config"]["providers"].as_mapping_mut().unwrap();
                    let entry = providers
                        .remove(Yaml::String("remote-codex".into()))
                        .unwrap();
                    providers.insert(Yaml::String("existing-gateway".into()), entry);
                }
                if row["id"].as_str() == Some("acp") {
                    row["config"]["provider"] = Yaml::String("existing-gateway".into());
                }
            }
            private_write(&path, serde_yaml::to_string(&doc).unwrap().as_bytes()).unwrap();
            let imported = import_live(&root, "Existing gateway").unwrap();
            assert_eq!(imported.base_url, p.base_url);
            assert_eq!(imported.api_key, p.api_key);
            assert_eq!(
                imported.settings_config["headers"]["X-Test"],
                "private-header"
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("dsh-home");
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","upstreams::dsh::tests::dsh_home_environment_is_isolated_and_live_import_tracks_the_selected_protocol"])
            .env(FLAG,"1").env("DSH_HOME",&root).env("HOME",temp.path()).env("CODEX_HOME",temp.path().join("codex")).env("CLAUDE_CONFIG_DIR",temp.path().join("claude")).output().unwrap();
        assert!(
            output.status.success(),
            "Isolated DSH child test failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    fn compatible_models_carry_real_facts_and_no_output_cap() {
        let meta = super::super::model_meta::ModelMeta {
            context_window: Some(272000),
            efforts: vec!["low".into(), "xhigh".into(), "max".into(), "ultra".into()],
            default_effort: Some("low".into()),
            input: vec!["text".into(), "image".into()],
        };
        let model = compatible_model("gpt-6-astra", None, None, Some(&meta), None);
        assert_eq!(
            model,
            json!({"id":"gpt-6-astra","name":"gpt-6-astra","contextWindow":272000,
                "reasoningEfforts":{"low":"low","xhigh":"xhigh","max":"max"},
                "input":["text","image"]})
        );
        // An explicit window wins; an earlier entry keeps what it had.
        let previous = json!({"id":"old","contextWindow":128000,"maxTokens":4096,"reasoningEfforts":{"high":"high"}});
        assert_eq!(
            compatible_model("x", Some(64000), None, Some(&meta), None)["contextWindow"],
            64000
        );
        let kept = compatible_model("old", None, None, None, Some(&previous));
        assert_eq!(
            kept,
            json!({"id":"old","name":"old","contextWindow":128000,"reasoningEfforts":{"high":"high"}})
        );
        assert_eq!(
            retry_policy()["retryableCodes"]
                .as_array()
                .unwrap()
                .last()
                .unwrap(),
            "PI_AI_ERROR"
        );
    }

    #[test]
    fn declared_upstream_facts_win_and_every_served_model_is_declared() {
        let declared: Vec<DiscoveredModel> = serde_json::from_value(json!([
            {"id":"qwen-local","name":"Qwen Local","contextWindow":200192,"maxOutputTokens":8192,
             "input":["text","image"],"reasoning":{"efforts":[
                {"reasoningEffort":"none","description":"None"},{"reasoningEffort":"low","description":"Low"},
                {"reasoningEffort":"xhigh","description":"Extra high"},{"reasoningEffort":"ultra","description":"Ultra"}],
                "default":"xhigh"}},
            {"id":"plain","name":"plain","reasoning":{"efforts":[],"default":null}},
            {"id":"unknown","name":"Unknown facts"}
        ]))
        .unwrap();
        let codex = super::super::model_meta::ModelMeta {
            context_window: Some(272000),
            efforts: vec!["high".into()],
            default_effort: Some("high".into()),
            input: vec!["text".into()],
        };
        assert_eq!(
            compatible_model("qwen-local", None, Some(&declared[0]), Some(&codex), None),
            json!({"id":"qwen-local","name":"Qwen Local","contextWindow":200192,
                "reasoningEfforts":{"off":"none","low":"low","xhigh":"xhigh"},"input":["text","image"]})
        );
        assert_eq!(
            compatible_model("plain", None, Some(&declared[1]), Some(&codex), None)
                ["reasoningEfforts"],
            json!(false)
        );
        // Unknown upstream facts still fall back to Codex's catalog.
        assert_eq!(
            compatible_model("unknown", None, Some(&declared[2]), Some(&codex), None),
            json!({"id":"unknown","name":"Unknown facts","contextWindow":272000,
                "reasoningEfforts":{"high":"high"},"input":["text"]})
        );

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("isolated-dsh-home");
        let store = temp.path().join("store");
        let mut p = super::super::tests::fixture("deepseek");
        p.api_type = "chat_completions".into();
        p.model = "unknown".into();
        p.models = declared;
        upsert(&store, p.clone()).unwrap();
        activate_at(&store, &p, &root).unwrap();
        let rows = yaml_doc(&root.join("profiles/acp/cordis.patch.yml"), "").unwrap();
        let pi = rows
            .as_sequence()
            .unwrap()
            .iter()
            .find(|v| v["id"].as_str() == Some("llm-pi-ai"))
            .unwrap();
        let route = serde_json::to_value(&pi["config"]["providers"]["remote-codex"]).unwrap();
        let ids: Vec<_> = route["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["unknown", "qwen-local", "plain"]);
        assert_eq!(route["api"], "openai-completions");
        assert_eq!(route["models"][1]["contextWindow"], 200192);
        assert!(route["models"][1].get("maxTokens").is_none());
    }

    #[test]
    fn switching_the_model_keeps_earlier_models_for_existing_threads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("isolated-dsh-home");
        let store = temp.path().join("store");
        let mut p = super::super::tests::fixture("deepseek");
        p.api_type = "responses".into();
        p.model = "first".into();
        upsert(&store, p.clone()).unwrap();
        activate_at(&store, &p, &root).unwrap();
        p.model = "second".into();
        upsert(&store, p.clone()).unwrap();
        activate_at(&store, &p, &root).unwrap();
        let rows = yaml_doc(&root.join("profiles/acp/cordis.patch.yml"), "").unwrap();
        let rows = rows.as_sequence().unwrap();
        let pi = rows
            .iter()
            .find(|v| v["id"].as_str() == Some("llm-pi-ai"))
            .unwrap();
        let ids: Vec<_> = pi["config"]["providers"]["remote-codex"]["models"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["second", "first"]);
        let route = &pi["config"]["providers"]["remote-codex"];
        // Responses routes send no output cap (the model's own limit applies)
        // and retry unclassified gateway failures with backoff.
        assert_eq!(
            route["compat"]["supportsMaxOutputTokens"].as_bool(),
            Some(false)
        );
        assert_eq!(route["retryPolicy"]["mode"].as_str(), Some("normal"));
        assert!(route["models"][0].get("maxTokens").is_none());
        let acp = rows
            .iter()
            .find(|v| v["id"].as_str() == Some("acp"))
            .unwrap();
        assert_eq!(acp["config"]["model"].as_str(), Some("second"));
    }

    #[test]
    fn dsh_official_and_openai_upstreams_preserve_other_routes_and_restore_private_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("isolated-dsh-home");
        let store = temp.path().join("store");
        let credentials =
            "version: 1\nrefs:\n  DEEPSEEK_API_KEY: original\n  OTHER_TOOL_KEY: preserve\n";
        let patch = "- id: unrelated\n  disabled: true\n- id: llm-pi-ai\n  config:\n    providers:\n      personal: {apiKeyEnv: OTHER_TOOL_KEY}\n";
        private_write(&root.join(".credentials.yaml"), credentials.as_bytes()).unwrap();
        private_write(
            &root.join("profiles/acp/cordis.patch.yml"),
            patch.as_bytes(),
        )
        .unwrap();
        let mut official = super::super::tests::fixture("deepseek");
        official.api_type = "anthropic".into();
        official.api_key = "synthetic-official-key".into();
        official.base_url = "https://api.deepseek.com/anthropic".into();
        let mut compatible = super::super::tests::fixture("deepseek");
        compatible.api_type = "chat_completions".into();
        compatible.api_key = "synthetic-compatible-key".into();
        for p in [&official, &compatible] {
            upsert(&store, p.clone()).unwrap();
            activate_at(&store, p, &root).unwrap();
        }
        let refs = yaml_doc(&root.join(".credentials.yaml"), "").unwrap();
        assert_eq!(refs["refs"]["OTHER_TOOL_KEY"].as_str(), Some("preserve"));
        assert_eq!(
            refs["refs"][credential_ref(&official)].as_str(),
            Some(official.api_key.as_str())
        );
        assert_eq!(
            refs["refs"][credential_ref(&compatible)].as_str(),
            Some(compatible.api_key.as_str())
        );
        let rows = yaml_doc(&root.join("profiles/acp/cordis.patch.yml"), "").unwrap();
        assert!(rows
            .as_sequence()
            .unwrap()
            .iter()
            .any(|v| v["id"].as_str() == Some("unrelated")));
        let pi = rows
            .as_sequence()
            .unwrap()
            .iter()
            .find(|v| v["id"].as_str() == Some("llm-pi-ai"))
            .unwrap();
        assert!(pi["config"]["providers"].get("personal").is_some());
        assert_eq!(
            pi["config"]["providers"]["remote-codex"]["baseURL"].as_str(),
            Some(compatible.base_url.as_str())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in paths(&compatible, &root).unwrap() {
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        remove(&store, &compatible.id).unwrap();
        assert_eq!(
            read(&root.join(".credentials.yaml")).unwrap().unwrap(),
            credentials
        );
        assert_eq!(
            read(&root.join("profiles/acp/cordis.patch.yml"))
                .unwrap()
                .unwrap(),
            patch
        );
    }
}
