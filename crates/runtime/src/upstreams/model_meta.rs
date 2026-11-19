//! Model facts that OpenAI-style gateways leave out of `/models`: context
//! window, reasoning levels and input kinds. Codex runs with a catalog that has
//! them for its models, and `codex debug models` prints it.
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelMeta {
    pub context_window: Option<i64>,
    /// Reasoning levels the model accepts, as Codex names them.
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub input: Vec<String>,
}

/// Models by id from `codex debug models` output.
pub fn parse(catalog: &Value) -> HashMap<String, ModelMeta> {
    catalog["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model["slug"].as_str()?.to_string();
            let strings = |value: &Value| -> Vec<String> {
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry.as_str().or_else(|| entry["effort"].as_str()))
                    .map(str::to_string)
                    .collect()
            };
            Some((
                id,
                ModelMeta {
                    context_window: model["context_window"].as_i64().filter(|n| *n > 0),
                    efforts: strings(&model["supported_reasoning_levels"]),
                    default_effort: model["default_reasoning_level"]
                        .as_str()
                        .map(str::to_string),
                    input: strings(&model["input_modalities"]),
                },
            ))
        })
        .collect()
}

const REFRESH: Duration = Duration::from_secs(600);
const RUN_TIMEOUT: Duration = Duration::from_secs(20);

/// Facts for one model id, when Codex knows it. Unknown models, a missing
/// Codex CLI or a failed run all mean "no facts" rather than an error.
pub fn lookup(model: &str) -> Option<ModelMeta> {
    static CACHE: OnceLock<Mutex<Option<(Instant, HashMap<String, ModelMeta>)>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    if cache.as_ref().is_none_or(|(at, _)| at.elapsed() > REFRESH) {
        let models = load().unwrap_or_else(|reason| {
            tracing::warn!(%reason, "Codex model facts unavailable; DSH upstream models keep their configured values");
            HashMap::new()
        });
        *cache = Some((Instant::now(), models));
    }
    cache.as_ref()?.1.get(model).cloned()
}

fn load() -> Result<HashMap<String, ModelMeta>, String> {
    // Tests and diagnostics can point at a saved catalog instead of Codex.
    if let Some(path) = std::env::var_os("POCKYMOE_MODEL_CATALOG") {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        return Ok(parse(
            &serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
        ));
    }
    if cfg!(test) {
        return Ok(HashMap::new());
    }
    // Several Codex installs can coexist; a newer CLI knows newer models.
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut installs: Vec<(Vec<u64>, std::path::PathBuf)> =
        which::which_in_all("codex", Some(crate::acp::child_path()), cwd)
            .map_err(|e| e.to_string())?
            .filter_map(|path| std::fs::canonicalize(&path).ok().map(|real| (path, real)))
            .fold(
                Vec::<(std::path::PathBuf, std::path::PathBuf)>::new(),
                |mut seen, (path, real)| {
                    if !seen.iter().any(|(_, known)| known == &real) {
                        seen.push((path, real));
                    }
                    seen
                },
            )
            .into_iter()
            .map(|(path, _)| {
                (
                    run(&path, &["--version"])
                        .map(|out| version(&out))
                        .unwrap_or_default(),
                    path,
                )
            })
            .collect();
    if installs.is_empty() {
        return Err("codex CLI not found".into());
    }
    installs.sort();
    let mut models = HashMap::new();
    let mut errors = Vec::new();
    for (_, codex) in &installs {
        match run(codex, &["debug", "models"]).and_then(|out| {
            serde_json::from_slice(&out).map_err(|e| format!("unreadable catalog: {e}"))
        }) {
            Ok(catalog) => models.extend(parse(&catalog)),
            Err(error) => errors.push(format!("{}: {error}", codex.display())),
        }
    }
    if models.is_empty() {
        return Err(errors.join("; "));
    }
    tracing::info!(
        installs = installs.len(),
        models = models.len(),
        "loaded Codex model facts"
    );
    Ok(models)
}

/// `codex-cli 0.160.0` → [0, 160, 0].
fn version(output: &[u8]) -> Vec<u64> {
    String::from_utf8_lossy(output)
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .split('.')
        .map(|part| part.trim().parse().unwrap_or(0))
        .collect()
}

/// `codex debug models` prints about 1 MB today; leave room for growth.
const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// RUN_TIMEOUT bounds the whole run: the process, its output and its pipe.
/// A descendant that keeps stdout open must not hold the model cache lock.
fn run(program: &std::path::Path, args: &[&str]) -> Result<Vec<u8>, String> {
    run_until(program, args, Instant::now() + RUN_TIMEOUT)
}

fn run_until(
    program: &std::path::Path,
    args: &[&str],
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new(program);
    command.args(args).env("PATH", crate::acp::child_path());
    let (status, bytes, truncated) =
        crate::child_process::run_bounded(&mut command, OUTPUT_LIMIT, deadline)
            .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("exited with {status}"));
    }
    if truncated {
        return Err(format!("output exceeded {OUTPUT_LIMIT} bytes"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_catalog_entries_become_model_facts() {
        let models = parse(&json!({"models":[
            {"slug":"gpt-6-astra","context_window":272000,"default_reasoning_level":"low",
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"max"},{"effort":"ultra"}],
             "input_modalities":["text","image"]},
            {"slug":"bare"}
        ]}));
        assert_eq!(
            models["gpt-6-astra"],
            ModelMeta {
                context_window: Some(272000),
                efforts: vec!["low".into(), "max".into(), "ultra".into()],
                default_effort: Some("low".into()),
                input: vec!["text".into(), "image".into()],
            }
        );
        assert_eq!(models["bare"], ModelMeta::default());
        assert_eq!(version(b"codex-cli 0.160.0\n"), vec![0, 160, 0]);
        assert!(version(b"codex-cli 0.154.0") < version(b"codex-cli 0.160.0"));
    }

    #[cfg(unix)]
    #[test]
    fn a_descendant_holding_stdout_cannot_outlast_the_run_deadline() {
        let started = Instant::now();
        let error = run_until(
            std::path::Path::new("sh"),
            &["-c", "sleep 30 & printf '{}'"],
            Instant::now() + Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(error.contains("output to close"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let error = run_until(
            std::path::Path::new("sh"),
            &["-c", "sleep 30"],
            Instant::now() + Duration::from_millis(300),
        )
        .unwrap_err();
        assert_eq!(error, "timed out");
        assert_eq!(
            run_until(
                std::path::Path::new("sh"),
                &["-c", "printf '{}'"],
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap(),
            b"{}"
        );
    }
}
