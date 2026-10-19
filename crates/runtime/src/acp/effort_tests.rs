// A session's effort option lists only its current model's levels.
use super::*;

fn options(model: &str, efforts: &[&str]) -> Value {
    json!([
        {"id": "model", "category": "model", "currentValue": model, "options": [
            {"value": "opus", "name": "Opus"},
            {"value": "sonnet", "name": "Sonnet"},
            {"value": "haiku", "name": "Haiku"}
        ]},
        {"id": "effort", "category": "thought_level", "currentValue": "medium",
         "options": efforts.iter().map(|e| json!({"value": e, "name": e})).collect::<Vec<_>>()}
    ])
}

fn efforts(models: &[ModelOptionDto], id: &str) -> Vec<String> {
    let model = models.iter().find(|m| m.model == id).unwrap();
    model
        .supported_reasoning_efforts
        .iter()
        .map(|e| e.reasoning_effort.clone())
        .collect()
}

#[test]
fn each_model_offers_the_efforts_learned_for_it() {
    let runtime = AcpRuntime::catalog(None, 1_000);
    runtime.learn_efforts(
        "claude",
        &options("opus", &["low", "medium", "high", "xhigh", "max"]),
    );
    let session = options("sonnet", &["low", "medium", "high", "max"]);
    runtime.learn_efforts("claude", &session);
    let models = runtime.with_learned_efforts("claude", models_from_config_options(&session));
    assert_eq!(
        efforts(&models, "opus"),
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(efforts(&models, "sonnet"), ["low", "medium", "high", "max"]);
    // Not learned yet: the session's own list stands in.
    assert_eq!(efforts(&models, "haiku"), ["low", "medium", "high", "max"]);
    // What one agent learned is not another's.
    let other = runtime.with_learned_efforts("codex", models_from_config_options(&session));
    assert_eq!(efforts(&other, "opus"), ["low", "medium", "high", "max"]);
}

#[test]
fn a_probe_visits_each_unknown_model_once() {
    let runtime = AcpRuntime::catalog(None, 1_000);
    let session = options("sonnet", &["low", "medium", "high", "max"]);
    runtime.learn_efforts("claude", &session);
    let models = models_from_config_options(&session);
    assert_eq!(runtime.claim_unprobed("claude", &models), ["opus", "haiku"]);
    assert!(runtime.claim_unprobed("claude", &models).is_empty());
}

#[test]
fn a_model_without_effort_levels_is_learned_as_such() {
    let runtime = AcpRuntime::catalog(None, 1_000);
    runtime.learn_efforts("claude", &options("haiku", &[]));
    let models = runtime.with_learned_efforts(
        "claude",
        models_from_config_options(&options("opus", &["low", "high"])),
    );
    assert!(efforts(&models, "haiku").is_empty());
    assert_eq!(
        models
            .iter()
            .find(|m| m.model == "haiku")
            .unwrap()
            .default_reasoning_effort,
        None
    );
}
