//! Pull-based, bounded encrypted downloads. Each continuation is bound to the
//! authenticated principal that started it and retains its original route scope.
use super::*;
use axum::body::{Body, BodyDataStream, Bytes};
use futures_util::StreamExt;
use std::time::{Duration, Instant};
const CHUNK: usize = 1024 * 1024;
#[derive(Default)]
pub(crate) struct Streams(Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<Entry>>>>>);
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StreamPrincipal {
    Session(String),
    Peer(crate::owner_trust::PeerPrincipal),
    Unauthenticated,
}
impl StreamPrincipal {
    pub fn new(
        owner: Option<&crate::owner_trust::OwnerPrincipal>,
        peer: Option<&crate::owner_trust::PeerPrincipal>,
    ) -> Self {
        match (owner, peer) {
            (Some(owner), _) => Self::Session(owner.session_id.clone()),
            (_, Some(peer)) => Self::Peer(peer.clone()),
            _ => Self::Unauthenticated,
        }
    }
}
struct Entry {
    body: BodyDataStream,
    pending: Bytes,
    last: Option<(u64, Bytes, bool)>,
    next: u64,
    path: String,
    principal: StreamPrincipal,
    used: Instant,
}
/// Continuations have an exact shape. A malformed scoped ID must never become
/// the generic /api scope used by legitimate device-level downloads.
fn continuation_scope(path: &str) -> Result<String> {
    let parts: Vec<_> = path.split('?').next().unwrap_or("").split('/').collect();
    match parts.as_slice() {
        ["", "api", "transport", "stream", id] if !id.is_empty() => Ok("/api".into()),
        ["", "api", "peer", "transport", "stream", id] if !id.is_empty() => Ok("/api/peer".into()),
        ["", "api", resource @ ("threads" | "workspaces"), id, "transport", "stream", stream]
            if Uuid::parse_str(id).is_ok() && !stream.is_empty() =>
        {
            Ok(format!("/api/{resource}/{id}"))
        }
        _ => bail!("invalid download scope"),
    }
}
fn scope(path: &str) -> String {
    let parts: Vec<_> = path.split('?').next().unwrap_or("").split('/').collect();
    if parts.get(2) == Some(&"peer") {
        "/api/peer".into()
    } else if parts.len() > 3
        && matches!(parts[2], "threads" | "workspaces")
        && Uuid::parse_str(parts[3]).is_ok()
    {
        format!("/api/{}/{}", parts[2], parts[3])
    } else {
        "/api".into()
    }
}
impl Entry {
    async fn read(&mut self, index: u64) -> Result<(Bytes, bool)> {
        self.used = Instant::now();
        if let Some((previous, bytes, done)) = &self.last {
            if *previous == index {
                return Ok((bytes.clone(), *done));
            }
        }
        if index != self.next {
            bail!("invalid download sequence");
        }
        let mut out = Vec::with_capacity(CHUNK);
        let mut done = false;
        while out.len() < CHUNK {
            if !self.pending.is_empty() {
                let count = (CHUNK - out.len()).min(self.pending.len());
                out.extend_from_slice(&self.pending.split_to(count));
                continue;
            }
            match tokio::time::timeout(Duration::from_secs(30), self.body.next()).await? {
                Some(Ok(bytes)) => self.pending = bytes,
                Some(Err(error)) => return Err(error.into()),
                None => {
                    done = true;
                    break;
                }
            }
        }
        if !done && self.pending.is_empty() {
            match tokio::time::timeout(Duration::from_secs(30), self.body.next()).await? {
                Some(Ok(bytes)) => self.pending = bytes,
                Some(Err(error)) => return Err(error.into()),
                None => done = true,
            }
        }
        let bytes = Bytes::from(out);
        self.last = Some((index, bytes.clone(), done));
        self.next += 1;
        Ok((bytes, done))
    }
}
impl Streams {
    pub async fn begin(
        &self,
        path: &str,
        body: Body,
        principal: StreamPrincipal,
    ) -> Result<(Bytes, Option<String>)> {
        let mut entry = Entry {
            body: body.into_data_stream(),
            pending: Bytes::new(),
            last: None,
            next: 0,
            path: scope(path),
            principal,
            used: Instant::now(),
        };
        let (bytes, done) = entry.read(0).await?;
        if done {
            return Ok((bytes, None));
        }
        let mut entries = self.0.lock().map_err(|_| anyhow!("download lock failed"))?;
        entries.retain(|_, entry| {
            entry
                .try_lock()
                .map_or(true, |e| e.used.elapsed() < Duration::from_secs(120))
        });
        if entries.len() >= 16 {
            bail!("too many active downloads");
        }
        let id = B64.encode(random());
        let next = format!("{}/transport/stream/{id}?chunk=1", entry.path);
        entries.insert(id.clone(), Arc::new(tokio::sync::Mutex::new(entry)));
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(120)).await;
                let Some(map) = weak.upgrade() else {
                    break;
                };
                let Ok(mut entries) = map.lock() else {
                    break;
                };
                let expired = entries.get(&id).is_none_or(|entry| {
                    entry
                        .try_lock()
                        .is_ok_and(|e| e.used.elapsed() >= Duration::from_secs(120))
                });
                if expired {
                    entries.remove(&id);
                    break;
                }
            }
        });
        Ok((bytes, Some(next)))
    }
    pub async fn read(&self, path: &str, principal: &StreamPrincipal) -> Result<Value> {
        let scope = continuation_scope(path)?;
        let url = url::Url::parse(&format!("http://device{path}"))?;
        let id = url.path().rsplit('/').next().unwrap_or("");
        let index = url
            .query_pairs()
            .find(|(key, _)| key == "chunk")
            .ok_or_else(|| anyhow!("missing chunk"))?
            .1
            .parse::<u64>()?;
        let entry = {
            let entries = self.0.lock().map_err(|_| anyhow!("download lock failed"))?;
            entries
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow!("download expired"))?
        };
        let mut entry = entry.lock().await;
        if entry.used.elapsed() > Duration::from_secs(120)
            || entry.path != scope
            || &entry.principal != principal
        {
            bail!("download expired or wrong scope or principal");
        }
        let (bytes, done) = entry.read(index).await?;
        let next =
            (!done).then(|| format!("{}/transport/stream/{id}?chunk={}", entry.path, index + 1));
        drop(entry);
        if done {
            self.0
                .lock()
                .map_err(|_| anyhow!("download lock failed"))?
                .remove(id);
        }
        // Intermediate reads are idempotent; after EOF the stream releases its file.
        Ok(
            json!({"statusCode":200,"headers":{"content-type":"application/octet-stream"},"body":base64::engine::general_purpose::STANDARD.encode(bytes),"bodyEncoding":"base64","streamNext":next}),
        )
    }
    pub fn expire(&self) {
        if let Ok(mut entries) = self.0.lock() {
            entries.retain(|_, entry| {
                entry
                    .try_lock()
                    .map_or(true, |e| e.used.elapsed() < Duration::from_secs(120))
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn continuations_require_the_starting_session_and_literal_scope() {
        let streams = Streams::default();
        let owner = StreamPrincipal::Session("owner-session".into());
        let (_, next) = streams
            .begin(
                "/api/threads/import-candidates",
                Body::from(vec![17u8; CHUNK + 7]),
                owner.clone(),
            )
            .await
            .unwrap();
        let next = next.unwrap();
        for other in [
            StreamPrincipal::Session("grantee-session".into()),
            StreamPrincipal::Session("another-owner-session".into()),
            StreamPrincipal::Unauthenticated,
        ] {
            assert!(streams.read(&next, &other).await.is_err());
        }
        for prefix in [
            "/api/threads/zzz".into(),
            "/api/workspaces/zzz".into(),
            format!("/api/threads/{}", Uuid::new_v4()),
            "/api/threads/zzz/../..".into(),
        ] {
            assert!(streams
                .read(&next.replacen("/api", &prefix, 1), &owner)
                .await
                .is_err());
        }
        let last = streams.read(&next, &owner).await.unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(last["body"].as_str().unwrap())
                .unwrap(),
            vec![17u8; 7]
        );
    }
    #[tokio::test]
    async fn peer_download_scope_stays_under_peer_and_cannot_cross_owner_scope() {
        let streams = Streams::default();
        let (_, next) = streams
            .begin(
                "/api/peer/files/read?workspaceId=secret",
                Body::from(vec![3u8; CHUNK + 7]),
                StreamPrincipal::Unauthenticated,
            )
            .await
            .unwrap();
        let next = next.unwrap();
        assert!(next.starts_with("/api/peer/transport/stream/"));
        assert!(streams
            .read(
                &next.replacen("/api/peer", "/api", 1),
                &StreamPrincipal::Unauthenticated
            )
            .await
            .is_err());
        let last = streams
            .read(&next, &StreamPrincipal::Unauthenticated)
            .await
            .unwrap();
        assert!(last["streamNext"].is_null());
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(last["body"].as_str().unwrap())
                .unwrap(),
            vec![3u8; 7]
        );
        let (_, next) = streams
            .begin(
                "/api/config/runtime",
                Body::from(vec![4u8; CHUNK + 1]),
                StreamPrincipal::Unauthenticated,
            )
            .await
            .unwrap();
        assert!(streams
            .read(
                &next.unwrap().replacen("/api", "/api/peer", 1),
                &StreamPrincipal::Unauthenticated
            )
            .await
            .is_err());
    }
    #[tokio::test]
    async fn bounded_chunks_keep_scope_sequence_and_binary_bytes() {
        let streams = Streams::default();
        let scope = format!("/api/workspaces/{}", Uuid::new_v4());
        let bytes = (0..CHUNK * 2 + 9)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>();
        let (first, next) = streams
            .begin(
                &format!("{scope}/files/download"),
                Body::from(bytes.clone()),
                StreamPrincipal::Unauthenticated,
            )
            .await
            .unwrap();
        assert_eq!(first.len(), CHUNK);
        let next = next.unwrap();
        assert!(streams
            .read(
                &next.replace(&scope, "/api"),
                &StreamPrincipal::Unauthenticated
            )
            .await
            .is_err());
        assert!(streams
            .read(
                &next.replace("chunk=1", "chunk=2"),
                &StreamPrincipal::Unauthenticated
            )
            .await
            .is_err());
        let second = streams
            .read(&next, &StreamPrincipal::Unauthenticated)
            .await
            .unwrap();
        assert_eq!(
            streams
                .read(&next, &StreamPrincipal::Unauthenticated)
                .await
                .unwrap(),
            second
        );
        let third = streams
            .read(
                second["streamNext"].as_str().unwrap(),
                &StreamPrincipal::Unauthenticated,
            )
            .await
            .unwrap();
        assert!(third["streamNext"].is_null());
        let mut all = first.to_vec();
        all.extend(
            base64::engine::general_purpose::STANDARD
                .decode(second["body"].as_str().unwrap())
                .unwrap(),
        );
        all.extend(
            base64::engine::general_purpose::STANDARD
                .decode(third["body"].as_str().unwrap())
                .unwrap(),
        );
        assert_eq!(all, bytes);
        assert!(streams.0.lock().unwrap().is_empty());
        let (_, next) = streams
            .begin(
                &scope,
                Body::from(vec![0u8; CHUNK]),
                StreamPrincipal::Unauthenticated,
            )
            .await
            .unwrap();
        assert!(next.is_none());
    }
}
