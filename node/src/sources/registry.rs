use anyhow::Result;
use tracing::info;

use super::{test::TestSource, InputSource};

pub struct SourceRegistry {
    sources: Vec<Box<dyn InputSource>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self { sources: Vec::new() }
    }

    /// Discover available sources.
    ///
    /// In the current implementation this populates two `TestSource` instances.
    /// Future implementations will also scan for NDI and Decklink devices.
    pub fn scan(&mut self) -> Result<()> {
        self.sources.clear();

        let test_sources: Vec<Box<dyn InputSource>> = vec![
            Box::new(TestSource::default_config("test-1", "Test Source 1")?),
            Box::new(TestSource::default_config("test-2", "Test Source 2")?),
        ];

        let count = test_sources.len();
        self.sources.extend(test_sources);

        info!(count, "source scan complete");
        Ok(())
    }

    pub fn sources(&self) -> &[Box<dyn InputSource>] {
        &self.sources
    }

    pub fn get(&self, id: &str) -> Option<&dyn InputSource> {
        self.sources.iter().find(|s| s.id() == id).map(|s| s.as_ref())
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut dyn InputSource> {
        let pos = self.sources.iter().position(|s| s.id() == id)?;
        Some(self.sources[pos].as_mut())
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
