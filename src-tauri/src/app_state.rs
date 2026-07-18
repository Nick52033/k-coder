use std::time::Instant;

pub struct AppState {
    started_at: Instant,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
