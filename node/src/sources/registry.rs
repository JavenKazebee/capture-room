use anyhow::Result;
use tracing::info;

use super::{test::{TestSource, TestSourceConfig}, InputSource};

pub struct SourceRegistry {
    sources: Vec<Box<dyn InputSource>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self { sources: Vec::new() }
    }

    /// Rebuild the source list from the provided test source configs.
    ///
    /// Future implementations will also scan for NDI and Decklink devices.
    /// Calling scan replaces all existing source instances; connected state
    /// is not preserved across a scan.
    pub fn scan(&mut self, configs: &[TestSourceConfig]) -> Result<()> {
        self.sources.clear();

        for cfg in configs {
            match TestSource::new(cfg.clone()) {
                Ok(src) => self.sources.push(Box::new(src)),
                Err(e) => tracing::warn!(id = %cfg.id, error = %e, "failed to create test source"),
            }
        }

        let count = self.sources.len();
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

    pub fn connect(&mut self, id: &str) -> Result<bool> {
        match self.get_mut(id) {
            Some(src) => {
                src.connect()?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn disconnect(&mut self, id: &str) -> bool {
        match self.get_mut(id) {
            Some(src) => {
                src.disconnect();
                true
            }
            None => false,
        }
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
