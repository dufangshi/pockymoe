//! Who sent a prompt when it was not the device owner. Only verified identities
//! count: the signed browser of a grantee, or on a device that is not bound to an
//! account key, the relay account the relay authorized.
use crate::owner_trust::BrowserPrincipal;
use axum::http::Extensions;
use pockymoe_protocol::MessageSenderDto;
use pockymoe_runtime::Supervisor;
use serde_json::Value;

/// The relay account behind a request that carried no signed browser identity.
#[derive(Clone, Debug)]
pub(crate) struct RelayActor {
    pub user_id: String,
    /// Older relays do not say; an unknown actor is not marked.
    pub owner: Option<bool>,
    pub name: Option<String>,
}

impl RelayActor {
    /// From the relay's `x-rcd-file-actor` and `x-rcd-actor-*` headers.
    pub(crate) fn from_headers(headers: &Value) -> Option<Self> {
        let user_id = headers["x-rcd-file-actor"]
            .as_str()
            .filter(|s| !s.is_empty())?;
        Some(Self {
            user_id: user_id.to_owned(),
            owner: headers["x-rcd-actor-owner"]
                .as_str()
                .map(|value| value == "1"),
            name: headers["x-rcd-actor-name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| name.chars().take(256).collect()),
        })
    }
}

pub(crate) fn sender(state: &Supervisor, extensions: &Extensions) -> Option<MessageSenderDto> {
    if let Some(principal) = extensions.get::<BrowserPrincipal>() {
        let user_id = principal.grantee_user_id.as_ref()?;
        return Some(MessageSenderDto {
            user_id: user_id.clone(),
            name: crate::grants::grantee_label(state, user_id),
        });
    }
    let actor = extensions.get::<RelayActor>()?;
    (actor.owner == Some(false)).then(|| MessageSenderDto {
        user_id: actor.user_id.clone(),
        name: actor.name.clone(),
    })
}
