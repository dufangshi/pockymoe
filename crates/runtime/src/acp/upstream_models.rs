use crate::upstreams::{self, DiscoveredModel, Profile};
use anyhow::Result;
use pockymoe_protocol::ModelOptionDto;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

type Cached = (Vec<u8>, Instant, Vec<DiscoveredModel>);
pub(super) struct UpstreamModels {
    directory: PathBuf,
    cache: Mutex<HashMap<String, Cached>>,
    native: Mutex<Option<(Vec<u8>, Instant, Vec<ModelOptionDto>)>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_probe_cache_is_revision_bound_and_invalidated_with_harness() {
        let temp = tempfile::tempdir().unwrap();
        let models = UpstreamModels::new(temp.path().into());
        models.cache_native(vec![1], vec![]).await;
        assert!(models.native_models(&[1]).await.is_some());
        assert!(models.native_models(&[2]).await.is_none());
        models.invalidate("grok").await;
        assert!(models.native_models(&[1]).await.is_none());
    }
}
impl UpstreamModels {
    pub fn native_oauth_selected(&self, harness: &str) -> bool {
        upstreams::native::oauth_selected(&self.directory, harness)
    }

    pub fn revision(profile: &Profile, models: &[DiscoveredModel]) -> Result<Vec<u8>> {
        Ok(Sha256::digest(serde_json::to_vec(&(profile, models))?).to_vec())
    }
    pub fn prepare_grok(&self, profile: &Profile, models: &[DiscoveredModel]) -> Result<()> {
        upstreams::prepare_grok_models(&self.directory, profile, models)
    }
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            cache: Mutex::new(HashMap::new()),
            native: Mutex::new(None),
        }
    }
    pub async fn invalidate(&self, harness: &str) {
        self.cache.lock().await.remove(harness);
        if harness == "grok" {
            *self.native.lock().await = None;
        }
    }
    pub async fn native_models(&self, revision: &[u8]) -> Option<Vec<ModelOptionDto>> {
        self.native
            .lock()
            .await
            .as_ref()
            .filter(|(key, time, _)| key == revision && time.elapsed() < Duration::from_secs(60))
            .map(|(_, _, models)| models.clone())
    }
    pub async fn cache_native(&self, revision: Vec<u8>, models: Vec<ModelOptionDto>) {
        *self.native.lock().await = Some((revision, Instant::now(), models));
    }
    pub async fn catalog(&self, harness: &str) -> Result<Option<(Profile, Vec<DiscoveredModel>)>> {
        let Some(profile) = upstreams::active_profile(&self.directory, harness)? else {
            return Ok(None);
        };
        let fingerprint = Sha256::digest(serde_json::to_vec(&profile)?).to_vec();
        let mut cache = self.cache.lock().await;
        if let Some((key, time, models)) = cache.get(harness) {
            if *key == fingerprint && time.elapsed() < Duration::from_secs(60) {
                return Ok(Some((profile, models.clone())));
            }
        }
        let result = tokio::time::timeout(
            Duration::from_secs(25),
            upstreams::discover_models(&profile),
        )
        .await??;
        let models: Vec<DiscoveredModel> = serde_json::from_value(result["models"].clone())?;
        cache.insert(
            harness.into(),
            (fingerprint, Instant::now(), models.clone()),
        );
        Ok(Some((profile, models)))
    }
}
