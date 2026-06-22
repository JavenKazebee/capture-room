use std::sync::{Arc, Mutex};

/// Holds the most recent thumbnail JPEG produced by the pipeline.
/// Clone is cheap — all clones share the same underlying storage.
#[derive(Debug, Clone, Default)]
pub struct ThumbnailStore {
    inner: Arc<Mutex<Option<Vec<u8>>>>,
}

impl ThumbnailStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&self, jpeg: Vec<u8>) {
        *self.inner.lock().unwrap() = Some(jpeg);
    }

    /// Returns a copy of the latest JPEG bytes, or `None` if no frame yet.
    pub fn latest(&self) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().clone()
    }
}
