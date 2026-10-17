use super::*;

fn put(root: &Path, name: &str, value: Value) {
    private_write(
        &root.join(name),
        serde_json::to_vec_pretty(&value).unwrap().as_slice(),
    )
    .unwrap();
}
fn text(root: &Path, name: &str, value: &str) {
    private_write(&root.join(name), value.as_bytes()).unwrap();
}
fn inventory_one(dir: &Path, h: &str, root: &Path) -> Value {
    inventory_at(dir, &BTreeMap::from([(h.into(), root.into())])).unwrap()[h].clone()
}
fn api(dir: &Path, h: &str, root: &Path, model: &str) -> Profile {
    let mut p = super::super::tests::fixture(h);
    p.model = model.into();
    if h == "deepseek" {
        p.api_type = "anthropic".into();
    }
    upsert_at(dir, p.clone(), root).unwrap();
    super::super::activate_at(dir, &p, root).unwrap();
    p
}
fn oauth_fixture(h: &str, root: &Path) {
    match h {
        "claude" => {
            put(
                root,
                "settings.json",
                json!({"hooks":{"new":true},"env":{"UNRELATED":"keep"},"model":"native-model"}),
            );
            put(
                root,
                ".credentials.json",
                json!({"claudeAiOauth":{"accessToken":"fixture-access","refreshToken":"fixture-refresh","expiresAt":1}}),
            );
        }
        "codex" => {
            text(
                root,
                "config.toml",
                "model = 'native-model'\n[mcp_servers.original]\ncommand = 'fixture'\n",
            );
            put(
                root,
                "auth.json",
                json!({"auth_mode":"chatgpt","tokens":{"access_token":"fixture-access","refresh_token":"fixture-refresh","account_id":"fixture-account"},"unrelated":"keep"}),
            );
        }
        "gemini" => {
            put(
                root,
                "settings.json",
                json!({"security":{"auth":{"selectedType":"oauth-personal"}},"model":{"name":"native-model"},"mcpServers":{"original":{}}}),
            );
            put(
                root,
                "oauth_creds.json",
                json!({"access_token":"fixture-access","refresh_token":"fixture-refresh","expiry_date":1}),
            );
            text(root, ".env", "UNRELATED=keep\n");
        }
        "grok" => {
            text(
                root,
                "config.toml",
                "[models]\ndefault='native-model'\n[mcp_servers.original]\ncommand='fixture'\n",
            );
            put(
                root,
                "auth.json",
                json!({"https://api.x.ai":{"auth_mode":"oidc","key":"fixture-access","refresh_token":"fixture-refresh","issuer":"https://fixture.invalid","expires_at":"2000-01-01T00:00:00Z"}}),
            );
        }
        _ => unreachable!(),
    }
}
#[test]
fn empty_directories_and_expired_unrefreshable_logins_are_not_available() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("store");
    for h in HARNESSES {
        let root = temp.path().join(h);
        assert_eq!(inventory_one(&dir, h, &root)["available"], false, "{h}");
    }
    for h in ["claude", "codex", "gemini", "grok"] {
        let root = temp.path().join(h);
        oauth_fixture(h, &root);
        assert_eq!(inventory_one(&dir, h, &root)["available"], true, "{h}");
        match h {
            "claude" => put(
                &root,
                ".credentials.json",
                json!({"claudeAiOauth":{"accessToken":"fixture-expired","expiresAt":1}}),
            ),
            "codex" => put(
                &root,
                "auth.json",
                json!({"tokens":{"access_token":"fixture-expired"}}),
            ),
            "gemini" => put(
                &root,
                "oauth_creds.json",
                json!({"access_token":"fixture-expired","expiry_date":1}),
            ),
            "grok" => put(
                &root,
                "auth.json",
                json!({"scope":{"auth_mode":"oidc","key":"fixture-expired","expires_at":"2000-01-01T00:00:00Z"}}),
            ),
            _ => unreachable!(),
        }
        assert_eq!(inventory_one(&dir, h, &root)["available"], false, "{h}");
    }
}
#[test]
fn oauth_roundtrips_across_multiple_apis_and_restart_preserve_latest_user_configuration() {
    let temp = tempfile::tempdir().unwrap();
    for h in ["claude", "codex", "gemini", "grok"] {
        let root = temp.path().join(h);
        let dir = temp.path().join(format!("{h}-store"));
        oauth_fixture(h, &root);
        let initial = inventory_one(&dir, h, &root);
        assert_eq!(initial["kind"], "oauth");
        assert_eq!(initial["active"], true);
        assert_eq!(inventory_one(&dir, h, &root), initial);
        for model in ["native-model", "second-api"] {
            api(&dir, h, &root, model);
            let managed = inventory_one(&dir, h, &root);
            assert_eq!(managed["available"], true, "{h}");
            assert_eq!(managed["active"], false);
        }
        match h {
            "claude" => {
                let mut d = json_doc(&root.join("settings.json")).unwrap();
                d["hooks"] = json!({"external":"latest"});
                put(&root, "settings.json", d);
            }
            "gemini" => {
                let mut d = json_doc(&root.join("settings.json")).unwrap();
                d["mcpServers"]["external"] = json!({"command":"latest"});
                put(&root, "settings.json", d);
            }
            _ => {
                let mut d = toml_doc(&root.join("config.toml")).unwrap();
                d["mcp_servers"]["external"]["command"] = value("latest");
                text(&root, "config.toml", &d.to_string());
            }
        }
        let revision = inventory_one(&dir, h, &root)["revision"]
            .as_str()
            .unwrap()
            .to_owned();
        activate_at(&dir, h, &root, Some(&revision)).unwrap();
        assert!(active_profile(&dir, h).unwrap().is_none());
        let restored = inventory_one(&dir, h, &root);
        assert_eq!(restored["available"], true, "{h}");
        assert_eq!(restored["active"], true);
        let path = if matches!(h, "claude" | "gemini") {
            "settings.json"
        } else {
            "config.toml"
        };
        assert!(read(&root.join(path)).unwrap().unwrap().contains("latest"));
        match h {
            "claude" => {
                let d = json_doc(&root.join(path)).unwrap();
                assert!(d["env"].get("ANTHROPIC_API_KEY").is_none());
                assert_eq!(d["model"], "native-model");
            }
            "codex" => {
                assert_eq!(
                    credentials(&root.join("auth.json")).unwrap()["tokens"]["refresh_token"],
                    "fixture-refresh"
                );
                assert_eq!(
                    toml_doc(&root.join(path)).unwrap()["model"].as_str(),
                    Some("native-model")
                );
            }
            "gemini" => assert_eq!(
                json_doc(&root.join(path))
                    .unwrap()
                    .pointer("/security/auth/selectedType"),
                Some(&json!("oauth-personal"))
            ),
            "grok" => {
                let d = toml_doc(&root.join(path)).unwrap();
                assert_eq!(d["models"]["default"].as_str(), Some("native-model"));
                assert!(d
                    .get("model")
                    .and_then(|m| m.get("native-model"))
                    .and_then(|m| m.get("api_key"))
                    .is_none());
            }
            _ => unreachable!(),
        }
        // Re-open the on-disk store, apply again, and restore again.
        api(&dir, h, &root, "third-api");
        activate_at(&dir, h, &root, None).unwrap();
        assert_eq!(inventory_one(&dir, h, &root)["active"], true, "{h}");
    }
}
#[test]
fn codex_newer_refreshed_tokens_win_and_custom_provider_restores_without_mcp_loss() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("codex");
    let dir = t.path().join("store");
    oauth_fixture("codex", &root);
    api(&dir, "codex", &root, "first");
    put(
        &root,
        "auth.json",
        json!({"auth_mode":"apikey","OPENAI_API_KEY":"fixture-stale-api-key","tokens":{"refresh_token":"fixture-new-refresh","access_token":"fixture-new-access"},"external":"keep"}),
    );
    // A subsequent API activation must preserve those refreshed tokens before
    // it overwrites auth.json again, including after process restart.
    api(&dir, "codex", &root, "intermediate-api");
    activate_at(&dir, "codex", &root, None).unwrap();
    let auth = credentials(&root.join("auth.json")).unwrap();
    assert_eq!(auth["tokens"]["refresh_token"], "fixture-new-refresh");
    assert_eq!(auth["external"], "keep");
    assert_eq!(auth["auth_mode"], "chatgpt");
    assert!(auth["OPENAI_API_KEY"].is_null());
    text(&root,"config.toml","model_provider='original'\nmodel='native'\n[model_providers.original]\nbase_url='https://fixture.invalid'\nexperimental_bearer_token='fixture-native-key'\n[mcp_servers.new]\ncommand='keep'\n");
    put(&root, "auth.json", json!({}));
    api(&dir, "codex", &root, "second");
    activate_at(&dir, "codex", &root, None).unwrap();
    let d = toml_doc(&root.join("config.toml")).unwrap();
    assert_eq!(d["model_provider"].as_str(), Some("original"));
    assert!(d["mcp_servers"].get("new").is_some());
    assert!(d["model_providers"].get("remote_codex").is_none());
}
#[test]
fn config_native_is_harness_local_and_never_becomes_a_connection_or_catalog_item() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("claude");
    let dir = t.path().join("store");
    put(
        &root,
        "settings.json",
        json!({"env":{"ANTHROPIC_API_KEY":"fixture-original-key","ANTHROPIC_BASE_URL":"https://original.invalid"},"hooks":{"neverSnapshot":true}}),
    );
    let native = inventory_one(&dir, "claude", &root);
    assert_eq!(native["kind"], "config");
    let c:connections::Connection=serde_json::from_value(json!({"name":"API","protocol":"anthropic","baseUrl":"https://fixture.invalid","apiKey":"fixture-api-key","model":"test"})).unwrap();
    let c_json = connections::upsert(&dir, c).unwrap();
    let c = connections::get(&dir, c_json["id"].as_str().unwrap()).unwrap();
    connections::apply_at(&dir, &c, "claude", "test", &root).unwrap();
    assert!(connections::binding(&dir, "claude").unwrap().is_some());
    activate_at(&dir, "claude", &root, None).unwrap();
    assert!(connections::binding(&dir, "claude").unwrap().is_none());
    let list = connections::list(&dir).unwrap();
    assert_eq!(list["connections"].as_array().unwrap().len(), 1);
    let metadata = connections::metadata(&dir).unwrap().to_string();
    assert!(!metadata.contains("original"));
    assert!(!metadata.contains("native"));
    assert!(!metadata.contains("fixture-original-key"));
    let s = load(&dir).unwrap();
    assert!(!s.native["claude"]
        .fragment
        .to_string()
        .contains("neverSnapshot"));
    assert!(
        !inventory_one(&dir, "claude", &t.path().join("other"))["available"]
            .as_bool()
            .unwrap()
    );
}
#[test]
fn revision_conflicts_and_mid_commit_failures_never_discard_external_changes() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("codex");
    let dir = t.path().join("store");
    oauth_fixture("codex", &root);
    api(&dir, "codex", &root, "first");
    let rev = inventory_one(&dir, "codex", &root)["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut d = toml_doc(&root.join("config.toml")).unwrap();
    d["model"] = value("external");
    text(&root, "config.toml", &d.to_string());
    assert!(activate_at(&dir, "codex", &root, Some(&rev))
        .unwrap_err()
        .to_string()
        .contains("conflict"));
    let config = read(&root.join("config.toml")).unwrap();
    let profiles = read(&dir.join("profiles.json")).unwrap();
    let result = activate_with(&dir, "codex", &root, None, |i| {
        if i == 0 {
            put(&root, "auth.json", json!({"external":"concurrent"}));
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(read(&root.join("config.toml")).unwrap(), config);
    assert_eq!(read(&dir.join("profiles.json")).unwrap(), profiles);
    assert_eq!(
        credentials(&root.join("auth.json")).unwrap()["external"],
        "concurrent"
    );
    let auth = read(&root.join("auth.json")).unwrap();
    assert!(activate_with(&dir, "codex", &root, None, |i| {
        if i == 2 {
            bail!("fixture failure");
        }
        Ok(())
    })
    .is_err());
    assert_eq!(read(&root.join("config.toml")).unwrap(), config);
    assert_eq!(read(&root.join("auth.json")).unwrap(), auth);
    assert_eq!(read(&dir.join("profiles.json")).unwrap(), profiles);
    assert!(active_profile(&dir, "codex").unwrap().is_some());
}
#[test]
fn legacy_migration_uses_first_unmanaged_backup_and_merges_latest_user_settings() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("claude");
    let dir = t.path().join("store");
    oauth_fixture("claude", &root);
    api(&dir, "claude", &root, "first");
    api(&dir, "claude", &root, "second");
    let mut s = load(&dir).unwrap();
    s.native.clear();
    save(&dir, &s).unwrap();
    let mut d = json_doc(&root.join("settings.json")).unwrap();
    d["hooks"] = json!({"external":true});
    put(&root, "settings.json", d);
    assert_eq!(inventory_one(&dir, "claude", &root)["kind"], "oauth");
    activate_at(&dir, "claude", &root, None).unwrap();
    let d = json_doc(&root.join("settings.json")).unwrap();
    assert!(d["env"].get("ANTHROPIC_API_KEY").is_none());
    assert_eq!(d["hooks"]["external"], true);
    assert_eq!(d["model"], "native-model");
    // Without an explicitly unmanaged historical snapshot, no latest-API fallback.
    api(&dir, "claude", &root, "third");
    let mut s = load(&dir).unwrap();
    s.native.clear();
    s.backups.retain(|b| b.active.is_some());
    save(&dir, &s).unwrap();
    assert_eq!(inventory_one(&dir, "claude", &root)["available"], false);
}
#[test]
fn native_helper_requires_config_and_codex_env_key_requires_a_real_value() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("claude");
    let dir = t.path().join("store");
    put(
        &root,
        "settings.json",
        json!({"apiKeyHelper":"fixture-helper"}),
    );
    assert_eq!(inventory_one(&dir, "claude", &root)["kind"], "config");
    assert_eq!(inventory_one(&dir, "claude", &root)["available"], true);
    let codex = t.path().join("codex");
    text(&codex,"config.toml","model_provider='original'\n[model_providers.original]\nenv_key='POCKYMOE_NONEXISTENT_FIXTURE_KEY_97693'\n");
    assert_eq!(inventory_one(&dir, "codex", &codex)["available"], false);
}
#[test]
fn deepseek_native_restores_provider_fields_and_preserves_new_credentials_and_patch_options() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("dsh");
    let dir = t.path().join("store");
    text(
        &root,
        ".credentials.yaml",
        "version: 1\nrefs:\n  ORIGINAL_KEY: fixture-original\n",
    );
    text(&root,"profiles/acp/cordis.patch.yml","- id: acp\n  config:\n    provider: deepseek-official\n    model: native-model\n- id: llm-deepseek\n  config:\n    baseURL: https://original.invalid\n    apiKeyEnv: ORIGINAL_KEY\n");
    assert_eq!(inventory_one(&dir, "deepseek", &root)["available"], true);
    api(&dir, "deepseek", &root, "managed");
    let path = root.join("profiles/acp/cordis.patch.yml");
    let mut patch: Value = serde_yaml::from_str(&read(&path).unwrap().unwrap()).unwrap();
    patch[0]["config"]["newOption"] = json!(true);
    text(
        &root,
        "profiles/acp/cordis.patch.yml",
        &serde_yaml::to_string(&patch).unwrap(),
    );
    activate_at(&dir, "deepseek", &root, None).unwrap();
    let p = dsh::import_live(&root, "native").unwrap();
    assert_eq!(p.api_key, "fixture-original");
    assert_eq!(p.model, "native-model");
    let patch: Value = serde_yaml::from_str(&read(&path).unwrap().unwrap()).unwrap();
    assert_eq!(patch[0]["config"]["newOption"], true);
    assert!(read(&root.join(".credentials.yaml"))
        .unwrap()
        .unwrap()
        .contains("REMOTE_CODEX_DSH_"));
}

#[test]
fn persisted_native_oauth_selection_controls_child_environment_cleanup() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("claude");
    let dir = t.path().join("store");
    oauth_fixture("claude", &root);
    inventory_one(&dir, "claude", &root);
    assert!(!oauth_selected_at(&dir, "claude", &root));
    inventory_one(&dir, "claude", &root);
    assert!(!oauth_selected_at(&dir, "claude", &root));
    api(&dir, "claude", &root, "managed");
    assert!(!oauth_selected_at(&dir, "claude", &root));
    activate_at(&dir, "claude", &root, None).unwrap();
    assert!(oauth_selected_at(&dir, "claude", &root));
    inventory_one(&dir, "claude", &root);
    assert!(oauth_selected_at(&dir, "claude", &root));
    assert!(!oauth_selected_at(&dir, "codex", &root));
}
#[cfg(unix)]
#[test]
fn native_credential_subprocess_is_bounded_and_reaped() {
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "printf fixture"]);
    assert_eq!(
        bounded_output(&mut command, std::time::Duration::from_secs(1)).unwrap(),
        b"fixture"
    );
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "exec sleep 5"]);
    let start = std::time::Instant::now();
    assert!(bounded_output(&mut command, std::time::Duration::from_millis(20)).is_none());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}
#[test]
fn gemini_and_grok_native_api_configuration_restores_its_own_key_and_preserves_external_fields() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("gemini");
    let dir = t.path().join("gemini-store");
    put(
        &root,
        "settings.json",
        json!({"security":{"auth":{"selectedType":"gemini-api-key"}},"model":{"name":"native"}}),
    );
    text(&root,".env","GEMINI_API_KEY=fixture-original\nGOOGLE_GEMINI_BASE_URL=https://native.invalid\nUNRELATED=original\n");
    api(&dir, "gemini", &root, "managed");
    text(
        &root,
        ".env",
        &(read(&root.join(".env")).unwrap().unwrap() + "EXTERNAL=new\n"),
    );
    activate_at(&dir, "gemini", &root, None).unwrap();
    let env = read(&root.join(".env")).unwrap().unwrap();
    assert!(env.contains("fixture-original"));
    assert!(env.contains("EXTERNAL=new"));
    assert!(!env.contains("secret-fixture"));
    assert_eq!(inventory_one(&dir, "gemini", &root)["kind"], "config");
    let root = t.path().join("grok");
    let dir = t.path().join("grok-store");
    text(&root,"config.toml","[models]\ndefault='same-model'\n[model.same-model]\napi_key='fixture-original'\nbase_url='https://native.invalid'\n");
    api(&dir, "grok", &root, "same-model");
    let mut d = toml_doc(&root.join("config.toml")).unwrap();
    d["model"]["same-model"]["external_option"] = value("new");
    text(&root, "config.toml", &d.to_string());
    activate_at(&dir, "grok", &root, None).unwrap();
    let d = toml_doc(&root.join("config.toml")).unwrap();
    assert_eq!(
        d["model"]["same-model"]["api_key"].as_str(),
        Some("fixture-original")
    );
    assert_eq!(
        d["model"]["same-model"]["external_option"].as_str(),
        Some("new")
    );
    assert_eq!(inventory_one(&dir, "grok", &root)["kind"], "config");
}
#[test]
fn legacy_oauth_migration_for_codex_gemini_and_grok_uses_unmanaged_provider_fragments() {
    let t = tempfile::tempdir().unwrap();
    for h in ["codex", "gemini", "grok"] {
        let root = t.path().join(h);
        let dir = t.path().join(format!("{h}-store"));
        oauth_fixture(h, &root);
        api(&dir, h, &root, "managed");
        api(&dir, h, &root, "second");
        let mut s = load(&dir).unwrap();
        s.native.clear();
        save(&dir, &s).unwrap();
        let n = inventory_one(&dir, h, &root);
        assert_eq!(n["available"], true, "{h}");
        assert_eq!(n["kind"], "oauth", "{h}");
        activate_at(&dir, h, &root, None).unwrap();
        assert_eq!(inventory_one(&dir, h, &root)["active"], true, "{h}");
    }
}
#[test]
fn gemini_existing_oauth_and_adc_and_codex_explicit_unauthenticated_provider_are_detected() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("store");
    let root = t.path().join("gemini");
    put(
        &root,
        "oauth_creds.json",
        json!({"refresh_token":"fixture-refresh"}),
    );
    assert_eq!(inventory_one(&dir, "gemini", &root)["kind"], "oauth");
    assert_eq!(inventory_one(&dir, "gemini", &root)["available"], true);
    put(
        &root,
        "settings.json",
        json!({"security":{"auth":{"selectedType":"vertex-ai"}}}),
    );
    let adc = t.path().join("adc.json");
    put(
        t.path(),
        "adc.json",
        json!({"type":"service_account","client_email":"fixture@example.invalid","private_key":"fixture-private-key"}),
    );
    text(
        &root,
        ".env",
        &format!(
            "GOOGLE_APPLICATION_CREDENTIALS={}\n",
            serde_json::to_string(&adc.to_str().unwrap()).unwrap()
        ),
    );
    assert_eq!(inventory_one(&dir, "gemini", &root)["available"], true);
    assert_eq!(inventory_one(&dir, "gemini", &root)["kind"], "config");
    put(t.path(), "adc.json", json!({}));
    assert_eq!(inventory_one(&dir, "gemini", &root)["available"], false);
    let root = t.path().join("codex");
    text(&root,"config.toml","model_provider='local'\n[model_providers.local]\nbase_url='http://127.0.0.1:12345'\nrequires_openai_auth=false\n");
    assert_eq!(inventory_one(&dir, "codex", &root)["available"], true);
}
