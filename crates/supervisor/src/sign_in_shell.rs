//! Sign in to a harness from the Web: a terminal on this device that runs the
//! harness's catalog sign-in command (`codex login --device-auth`). The reader
//! completes the flow there; when the command ends, or the window closes and
//! terminates it, the harness is checked again.
use axum::{extract::State, http::StatusCode, Json};
use pockymoe_runtime::acp::AcpAgentDef;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::http::{err, map_err, ApiErr, AppState};

/// Only a harness id and a size: the command comes from the catalog, never
/// from the client.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SignInShellBody {
    agent_id: String,
    cols: Option<u16>,
    rows: Option<u16>,
}

/// `POST /api/shells/sign-in`. Owner only: `/api/shells/*` is a terminal route
/// (`access::terminal_route`), which no share or grant reaches, and a scoped
/// agent interface (Treer) never reaches it either.
pub(crate) async fn create(
    State(state): State<AppState>,
    scope: Option<axum::Extension<crate::treer::Scope>>,
    Json(body): Json<SignInShellBody>,
) -> Result<Json<Value>, ApiErr> {
    if scope.is_some() {
        return Err(err(
            StatusCode::FORBIDDEN,
            "forbidden",
            "A scoped agent interface cannot open a sign-in terminal.",
        ));
    }
    // The same rule as `capabilities.terminal` in the runtime config.
    if !cfg!(unix) {
        return Err(err(
            StatusCode::NOT_IMPLEMENTED,
            "terminal_unavailable",
            "This device has no Web terminal.",
        ));
    }
    let def = pockymoe_runtime::acp::builtin_agents(state.config.acp_command.as_deref())
        .into_iter()
        .find(|def| def.id == body.agent_id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "Unknown harness."))?;
    start(&state, &def, body.cols, body.rows).map(Json)
}

/// Start `def`'s sign-in terminal with the program its ACP launch would
/// resolve and the environment that launch would add.
pub(crate) fn start(
    state: &AppState,
    def: &AcpAgentDef,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<Value, ApiErr> {
    let (program, args, env) = pockymoe_runtime::management::sign_in_program(def)
        .map_err(|error| {
            err(
                StatusCode::CONFLICT,
                "harness_unavailable",
                format!("{error:#}"),
            )
        })?
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "no_sign_in_command",
                format!("{} has no sign-in command.", def.display_name),
            )
        })?;
    let cols = cols.unwrap_or(80);
    let rows = rows.unwrap_or(24);
    if cols == 0 || rows == 0 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Shell rows and columns must be positive.",
        ));
    }
    let runtime = tokio::runtime::Handle::current();
    let checker = state.clone();
    let id = def.id.clone();
    let on_exit = Box::new(move || {
        // Recorded before the window hears of the exit, so it never reads
        // the result of an earlier check.
        pockymoe_runtime::management::begin_sign_in_check(&checker, &id);
        runtime.spawn(async move {
            pockymoe_runtime::management::check_sign_in(&checker, &id).await;
        });
    });
    let home = pockymoe_runtime::config::home_dir();
    let (_, shell) = crate::shells::hub()
        .create_sign_in(
            crate::shells::ShellProgram {
                program,
                args,
                env,
                sign_in: def.id.clone(),
            },
            &home.to_string_lossy(),
            cols,
            rows,
            on_exit,
        )
        .map_err(map_err)?;
    Ok(json!({
        "agentId": def.id,
        "command": def.sign_in_command,
        "shell": shell,
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use std::time::{Duration, Instant};
    use tower::ServiceExt;

    async fn post(state: &AppState, body: Value) -> (StatusCode, Value) {
        let token = crate::auth::login(&state.config, "admin", "secret123")
            .unwrap()
            .0;
        let response = crate::router(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/shells/sign-in")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn refuses_an_unknown_harness_a_harness_without_a_command_and_a_command_line() {
        let (_dir, original) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let mut config = original.config.clone();
        drop(original);
        config.acp_command = Some("/bin/cat".into());
        config.database_url = _dir.path().join("sign-in.sqlite");
        let db = pockymoe_runtime::Database::open(&config.database_url).unwrap();
        let state = std::sync::Arc::new(pockymoe_runtime::Supervisor::new(config, db, vec![]));

        let (status, body) = post(&state, json!({"agentId": "no-such-harness"})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        // The custom ACP agent and Gemini CLI have no checked sign-in command.
        for agent in ["custom", "gemini"] {
            let (status, body) = post(&state, json!({"agentId": agent})).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{agent}: {body}");
            assert_eq!(body["code"], "no_sign_in_command", "{body}");
        }
        // A client cannot name the command.
        let (status, _) = post(
            &state,
            json!({"agentId": "codex", "command": "/bin/sh -c 'touch /tmp/x'"}),
        )
        .await;
        assert!(status.is_client_error(), "{status}");
    }

    #[test]
    fn the_route_is_a_terminal_route_no_share_or_grant_reaches() {
        use pockymoe_protocol::access;
        assert!(access::terminal_route("/api/shells/sign-in"));
        let mut shared = access::owner_access();
        shared.kind = "shared";
        shared.scope = "device".into();
        assert!(!access::access_allows(
            &shared,
            "POST",
            "/api/shells/sign-in"
        ));
        assert!(access::access_allows(
            &access::owner_access(),
            "POST",
            "/api/shells/sign-in"
        ));
    }

    fn wait<T>(mut probe: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(value) = probe() {
                return value;
            }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn runs_the_catalog_command_in_a_pty_and_checks_the_harness_when_it_ends() {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let script = _dir.path().join("fake-login");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf 'login-args:%s|%s tty:%s term:%s\\n' \"$1\" \"$2\" \"$(test -t 0 && echo yes)\" \"$TERM\"\nread code\nprintf 'code:%s\\n' \"$code\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let program = script.to_string_lossy().into_owned();
        let def = AcpAgentDef {
            id: "codex".into(),
            display_name: "OpenAI Codex".into(),
            description: String::new(),
            transport: "adapter".into(),
            base_command: program.clone(),
            server_command: "codex-acp".into(),
            install_command: None,
            model_list_command: None,
            sign_in_command: Some(format!("{program} login --device-auth")),
        };
        let started = start(&state, &def, Some(100), Some(30)).ok().unwrap();
        assert_eq!(started["agentId"], "codex");
        let shell = &started["shell"];
        assert_eq!(shell["signIn"], "codex");
        assert_eq!(shell["threadId"], "", "belongs to no thread");
        let id = shell["id"].as_str().unwrap().to_string();
        let hub = crate::shells::hub();
        let output = wait(|| {
            let (replay, _) = hub.replay(&id).ok()?;
            replay.contains("term:").then_some(replay)
        });
        assert!(
            output.contains("login-args:login|--device-auth tty:yes term:xterm-256color"),
            "{output:?}"
        );
        hub.write(&id, "ABCD-1234\r").unwrap();
        // The command ends by itself: the entry goes and the harness is checked.
        wait(|| hub.get(&id).is_err().then_some(()));
        let job = wait(|| {
            let job = state
                .management_jobs
                .lock()
                .unwrap()
                .get("codex")
                .cloned()?;
            (job["state"] != "running").then_some(job)
        });
        assert_eq!(job["action"], "signIn", "{job}");
        // The fake Codex runtime answers the model probe: signed in.
        assert_eq!(job["state"], "completed", "{job}");
        assert_eq!(job["signedIn"], true, "{job}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closing_the_window_terminates_the_login_and_a_new_one_replaces_an_open_one() {
        let (_dir, state) = crate::tunnel::tests::state_with_relay_url("http://127.0.0.1:1");
        let def = AcpAgentDef {
            id: "claude".into(),
            display_name: "Claude Agent".into(),
            description: String::new(),
            transport: "adapter".into(),
            base_command: "/bin/sh".into(),
            server_command: "claude-agent-acp".into(),
            install_command: None,
            model_list_command: None,
            sign_in_command: Some("/bin/sh -c 'sleep 30'".into()),
        };
        let hub = crate::shells::hub();
        let first = start(&state, &def, None, None).ok().unwrap()["shell"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let second = start(&state, &def, None, None).ok().unwrap()["shell"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(hub.get(&first).is_err(), "the earlier sign-in was ended");
        assert!(hub.get(&second).is_ok());
        hub.terminate(&second).unwrap();
        let job = wait(|| {
            let job = state
                .management_jobs
                .lock()
                .unwrap()
                .get("claude")
                .cloned()?;
            (job["state"] != "running").then_some(job)
        });
        assert_eq!(job["action"], "signIn", "{job}");
    }
}
