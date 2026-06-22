use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ChannelLevel {
    pub peak_db: f64,
    pub rms_db: f64,
}

#[derive(Debug, Clone)]
pub struct AudioLevelState {
    pub channels: Vec<ChannelLevel>,
}

/// Holds the most recent audio level reading from the pipeline.
/// Clone is cheap — all clones share the same underlying storage.
#[derive(Debug, Clone, Default)]
pub struct AudioMeter {
    inner: Arc<Mutex<Option<AudioLevelState>>>,
}

impl AudioMeter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&self, state: AudioLevelState) {
        *self.inner.lock().unwrap() = Some(state);
    }

    /// Returns the most recent level state, or `None` if no reading yet.
    pub fn latest(&self) -> Option<AudioLevelState> {
        self.inner.lock().unwrap().clone()
    }
}
