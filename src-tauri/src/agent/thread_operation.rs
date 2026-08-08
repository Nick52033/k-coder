use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Debug, Default)]
pub struct ThreadOperationGate {
    entries: Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

pub type ThreadOperationGuard = OwnedMutexGuard<()>;

impl ThreadOperationGate {
    pub async fn lock(&self, thread_id: &str) -> ThreadOperationGuard {
        let entry = {
            let mut entries = self.entries.lock().await;
            entries.retain(|_, entry| entry.strong_count() > 0);
            match entries.get(thread_id).and_then(Weak::upgrade) {
                Some(entry) => entry,
                None => {
                    let entry = Arc::new(Mutex::new(()));
                    entries.insert(thread_id.to_string(), Arc::downgrade(&entry));
                    entry
                }
            }
        };
        entry.lock_owned().await
    }

    #[cfg(test)]
    async fn entry_count(&self) -> usize {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, entry| entry.strong_count() > 0);
        entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serializes_one_thread_without_blocking_another() {
        let gate = Arc::new(ThreadOperationGate::default());
        let first = gate.lock("thread-a").await;
        let same_thread = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.lock("thread-a").await })
        };
        let other_thread = gate.lock("thread-b").await;
        assert!(!same_thread.is_finished());
        drop(other_thread);
        drop(first);
        let acquired = same_thread.await.unwrap();
        drop(acquired);
        assert_eq!(gate.entry_count().await, 0);
    }
}
