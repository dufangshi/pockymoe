use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use tokio_util::sync::CancellationToken;
fn pending() -> &'static Mutex<HashMap<String, CancellationToken>> {
    static P: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();
    P.get_or_init(Default::default)
}
pub struct Pending {
    key: String,
    pub token: CancellationToken,
}
impl Pending {
    pub fn begin(key: String) -> Option<Self> {
        let mut p = pending().lock().unwrap();
        if p.contains_key(&key) {
            return None;
        }
        let token = CancellationToken::new();
        p.insert(key.clone(), token.clone());
        Some(Self { key, token })
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        pending().lock().unwrap().remove(&self.key);
    }
}
pub fn cancel(key: &str) {
    if let Some(token) = pending().lock().unwrap().get(key) {
        token.cancel();
    }
}
