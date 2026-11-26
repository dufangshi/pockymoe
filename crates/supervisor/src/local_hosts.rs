//! DNS rebinding guard for a local Supervisor that answers without sign-in.
//!
//! A web page on a hostile name can point that name at this computer and then
//! call the open local API from the browser as same-origin. Such requests carry
//! the hostile name in `Host`. Without a passcode nothing else stops them, so
//! only names that cannot be rebound are answered.
//!
//! Allowed: any IP address (a page can only send an IP as `Host` when it was
//! loaded from that address, which is this server), `localhost` and
//! `*.localhost`, this computer's name with or without `.local`/`.lan`, the
//! configured bind name, the MCP issuer's name, and the names listed in
//! `POCKYMOE_ALLOWED_HOSTS` (comma separated; `*.example.ts.net` allows a
//! domain's subdomains).
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use pockymoe_runtime::Supervisor;
use std::sync::OnceLock;

pub(crate) const ENV: &str = "POCKYMOE_ALLOWED_HOSTS";

fn machine_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        hostname::get()
            .ok()
            .and_then(|name| name.into_string().ok())
            .unwrap_or_default()
            .to_ascii_lowercase()
    })
}

/// The name a request was addressed to, without its port; `None` when the
/// request names no host (non-browser clients).
fn requested(headers: &HeaderMap, uri: &Uri) -> Option<Result<url::Host, ()>> {
    let raw = match headers.get(axum::http::header::HOST) {
        Some(value) => value.to_str().map_err(|_| ()).map(str::to_owned),
        None => Ok(uri.authority()?.as_str().to_owned()),
    };
    Some(raw.and_then(|raw| {
        url::Url::parse(&format!("http://{raw}"))
            .ok()
            .filter(|url| url.path() == "/" && url.query().is_none())
            .and_then(|url| url.host().map(|host| host.to_owned()))
            .ok_or(())
    }))
}

fn name_allowed(state: &Supervisor, name: &str, listed: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    let machine = machine_name();
    if !machine.is_empty()
        && [String::new(), ".local".into(), ".lan".into()]
            .iter()
            .any(|suffix| name == format!("{machine}{suffix}"))
    {
        return true;
    }
    if name.eq_ignore_ascii_case(state.config.host.trim()) {
        return true;
    }
    if url::Url::parse(&crate::mcp::issuer(state))
        .ok()
        .and_then(|issuer| issuer.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|issuer| issuer == name)
    {
        return true;
    }
    listed
        .split(',')
        .map(|entry| entry.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .any(|entry| match entry.strip_prefix("*.") {
            Some(domain) => name.ends_with(&format!(".{domain}")),
            None => entry == name,
        })
}

/// The refusal to send instead of running the request, if any.
pub(crate) fn refuse(state: &Supervisor, headers: &HeaderMap, uri: &Uri) -> Option<Response> {
    let host = match requested(headers, uri) {
        None => return None,
        Some(Ok(host)) => host,
        Some(Err(())) => return Some(refusal("this request")),
    };
    let allowed = match &host {
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => true,
        url::Host::Domain(name) => {
            name_allowed(state, name, &std::env::var(ENV).unwrap_or_default())
        }
    };
    (!allowed).then(|| refusal(&host.to_string()))
}

fn refusal(name: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "code": "host_not_allowed",
            "message": format!(
                "This Pockymoe answers without a passcode, so it only accepts its IP addresses, localhost and this computer's name. To open it as {name}, add that name to {ENV} (comma separated, for example in ~/.remote-codex/local-supervisor.json) and restart Pockymoe, or set a passcode."
            ),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn host(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_str(value).unwrap());
        headers
    }

    #[tokio::test]
    async fn rebinding_names_are_refused_and_local_names_allowed() {
        let f = crate::isolation_tests::setup().await;
        let s = &f.state;
        let uri: Uri = "/api/workspaces".parse().unwrap();
        for ok in [
            "127.0.0.1:8787",
            "127.1.2.3",
            "[::1]:8787",
            "192.168.1.20:8787",
            "[fe80::1]:9",
            "localhost:8787",
            "LOCALHOST",
            "app.localhost:5173",
            "localhost.",
        ] {
            assert!(refuse(s, &host(ok), &uri).is_none(), "{ok}");
        }
        let machine = machine_name();
        if !machine.is_empty() {
            for suffix in ["", ".local", ".lan"] {
                let name = format!("{machine}{suffix}:8787");
                assert!(refuse(s, &host(&name), &uri).is_none(), "{name}");
            }
        }
        for bad in [
            "evil.example:8787",
            "evil.example",
            "127.0.0.1.evil.example:8787",
            "localhost.evil.example",
            "ubuntu.attacker.test",
            "a b",
            "127.0.0.1:8787/path",
        ] {
            let refused = refuse(s, &host(bad), &uri).unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{bad}");
        }
        // No Host and no authority: not a browser.
        assert!(refuse(s, &HeaderMap::new(), &uri).is_none());
        // HTTP/2 carries the name in the authority instead.
        let h2: Uri = "http://evil.example:8787/api/workspaces".parse().unwrap();
        assert!(refuse(s, &HeaderMap::new(), &h2).is_some());
    }

    #[tokio::test]
    async fn listed_names_and_wildcards_are_allowed() {
        let f = crate::isolation_tests::setup().await;
        let listed = "box, *.tailnet.ts.net ,Pocky.Example.";
        for ok in ["box", "laptop.tailnet.ts.net", "pocky.example"] {
            assert!(name_allowed(&f.state, ok, listed), "{ok}");
        }
        for bad in ["tailnet.ts.net", "evil-tailnet.ts.net", "boxx", "box.evil"] {
            assert!(!name_allowed(&f.state, bad, listed), "{bad}");
        }
    }

    async fn call(
        state: &std::sync::Arc<Supervisor>,
        method: &str,
        path: &str,
        host_name: &str,
        relay: bool,
    ) -> (StatusCode, serde_json::Value) {
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", host_name)
            .header("content-type", "application/json");
        if relay {
            request = request.extension(crate::auth::TrustedRelayForward);
        }
        let body = if method == "POST" {
            r#"{"operation":"list"}"#
        } else {
            ""
        };
        let response = crate::http::router(state.clone())
            .oneshot(request.body(axum::body::Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    #[tokio::test]
    async fn the_open_local_api_refuses_rebound_names_on_every_route() {
        let f = crate::isolation_tests::setup().await;
        let s = &f.state;
        for (method, path) in [
            ("GET", "/api/workspaces"),
            ("GET", "/ws"),
            ("GET", "/"),
            ("GET", "/healthz"),
            ("POST", "/api/cli"),
            ("POST", "/api/mcp/consent/x"),
        ] {
            let (status, body) = call(s, method, path, "evil.example:8787", false).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
            assert_eq!(body["code"], "host_not_allowed", "{method} {path}");
            assert!(body["message"].as_str().unwrap().contains(ENV));
            let (_, body) = call(s, method, path, "127.0.0.1:8787", false).await;
            assert_ne!(body["code"], "host_not_allowed", "{method} {path}");
        }
        let (status, _) = call(s, "GET", "/api/workspaces", "localhost:8787", false).await;
        assert_eq!(status, StatusCode::OK);
        // The relay tunnel never carries a browser page's rebound name.
        let (_, body) = call(s, "GET", "/api/workspaces", "evil.example", true).await;
        assert_ne!(body["code"], "host_not_allowed");
        // With a passcode, sign-in (origin-bound cookies) decides as before.
        crate::auth::set_passcode(s, Some("open sesame")).unwrap();
        let (status, body) = call(s, "GET", "/api/workspaces", "evil.example", false).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_ne!(body["code"], "host_not_allowed");
    }
}
