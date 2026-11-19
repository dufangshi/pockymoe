use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use pockymoe_protocol::ThreadTurnDto;
use tokio::sync::Mutex;

type Fingerprint = Option<(PathBuf, u64, Option<SystemTime>)>;
type CachedHistory = (Fingerprint, Option<Arc<Vec<ThreadTurnDto>>>);

type SessionKey = (PathBuf, String, bool);
type HistorySlot = Arc<Mutex<Option<CachedHistory>>>;

/// Cache successful and unavailable usage scans. Large existing rollouts are read
/// off the async executor and unchanged files are never reparsed on page refresh.
#[derive(Default)]
pub(crate) struct UsageHistoryCache {
    entries: Mutex<VecDeque<(SessionKey, HistorySlot)>>,
}

impl UsageHistoryCache {
    async fn slot(&self, key: SessionKey) -> HistorySlot {
        let mut entries = self.entries.lock().await;
        let slot = entries
            .iter()
            .position(|(k, _)| k == &key)
            .and_then(|i| entries.remove(i))
            .map(|(_, slot)| slot)
            .unwrap_or_default();
        if entries.len() == 32 {
            entries.pop_front();
        }
        entries.push_back((key, slot.clone()));
        slot
    }

    pub(crate) async fn get(
        &self,
        home: &Path,
        session_id: &str,
    ) -> Option<Arc<Vec<ThreadTurnDto>>> {
        self.get_for(home, session_id, false).await
    }

    pub(crate) async fn get_grok(
        &self,
        home: &Path,
        session_id: &str,
    ) -> Option<Arc<Vec<ThreadTurnDto>>> {
        self.get_for(home, session_id, true).await
    }

    async fn get_for(
        &self,
        home: &Path,
        session_id: &str,
        grok: bool,
    ) -> Option<Arc<Vec<ThreadTurnDto>>> {
        let key = (
            home.to_path_buf(),
            crate::import_id::parse_session_ref(session_id).raw_id,
            grok,
        );
        let search = key.clone();
        let slot = self.slot(key).await;
        let mut entry = slot.lock().await;
        let fingerprint = tokio::task::spawn_blocking(move || {
            let path = if grok {
                crate::local_sessions::find_grok_updates(&search.0, &search.1)
            } else {
                crate::local_sessions::find_codex_rollout(&search.0, &search.1)
            }?;
            let meta = std::fs::metadata(&path).ok()?;
            Some((path, meta.len(), meta.modified().ok()))
        })
        .await
        .ok()?;
        if let Some((cached_fingerprint, history)) = entry.as_ref() {
            if cached_fingerprint == &fingerprint {
                return history.clone();
            }
        }
        let history = if let Some((path, _, _)) = &fingerprint {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                if grok {
                    crate::local_sessions::read_grok_usage_history(&path).map(Arc::new)
                } else {
                    crate::local_sessions::read_codex_usage_history(&path).map(Arc::new)
                }
            })
            .await
            .ok()
            .flatten()
        } else {
            None
        };
        *entry = Some((fingerprint, history.clone()));
        history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn refreshing_one_history_does_not_block_or_evict_a_recent_peer() {
        let cache = UsageHistoryCache::default();
        let key = |n: usize| (PathBuf::from("home"), n.to_string(), false);
        let loading = cache.slot(key(0)).await;
        let _guard = loading.lock().await;
        let hot = cache.slot(key(1)).await;
        for n in 2..32 {
            cache.slot(key(n)).await;
        }
        assert!(Arc::ptr_eq(&hot, &cache.slot(key(1)).await));
        cache.slot(key(32)).await;
        assert!(Arc::ptr_eq(&hot, &cache.slot(key(1)).await));
    }
}
