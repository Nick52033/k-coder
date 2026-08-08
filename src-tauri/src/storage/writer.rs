use std::collections::HashMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread::{self, JoinHandle};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, oneshot};

use super::StorageError;

const WRITER_CHANNEL_CAPACITY: usize = 64;

pub struct ThreadWriters {
    writers: Mutex<HashMap<String, WriterHandle>>,
    append_gates: AsyncMutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    next_id: AtomicU64,
}

impl fmt::Debug for ThreadWriters {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let active_writers = self.writers.lock().map(|writers| writers.len()).ok();
        formatter
            .debug_struct("ThreadWriters")
            .field("active_writers", &active_writers)
            .finish()
    }
}

struct WriterHandle {
    id: u64,
    sender: mpsc::SyncSender<WriteRequest>,
    join: JoinHandle<()>,
}

struct WriteRequest {
    line: Vec<u8>,
    acknowledged: oneshot::Sender<Result<(), StorageError>>,
}

impl ThreadWriters {
    pub fn new() -> Self {
        Self {
            writers: Mutex::new(HashMap::new()),
            append_gates: AsyncMutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    pub async fn lock_thread(&self, thread_id: &str) -> OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.append_gates.lock().await;
            gates.retain(|_, gate| gate.strong_count() > 0);
            match gates.get(thread_id).and_then(Weak::upgrade) {
                Some(gate) => gate,
                None => {
                    let gate = Arc::new(AsyncMutex::new(()));
                    gates.insert(thread_id.to_string(), Arc::downgrade(&gate));
                    gate
                }
            }
        };
        gate.lock_owned().await
    }

    pub async fn append(
        &self,
        thread_id: &str,
        path: PathBuf,
        line: Vec<u8>,
    ) -> Result<(), StorageError> {
        let (mut request, acknowledged) = WriteRequest::new(line);
        for attempt in 0..2 {
            let (writer_id, sender) = self.writer(thread_id, path.clone())?;
            let (sent, returned) =
                tokio::task::spawn_blocking(move || match sender.send(request) {
                    Ok(()) => (true, None),
                    Err(error) => (false, Some(error.0)),
                })
                .await
                .map_err(|error| StorageError::Io(error.to_string()))?;
            if sent {
                debug_assert!(returned.is_none());
                return acknowledged.await.map_err(|_| {
                    StorageError::Io("thread writer stopped before durable ack".into())
                })?;
            }

            request = returned.expect("a disconnected writer returns the write request");
            self.remove_if_current(thread_id, writer_id)?;
            if attempt == 1 {
                return Err(StorageError::Io(format!(
                    "thread writer for {thread_id} stopped before accepting the event"
                )));
            }
        }
        unreachable!("writer send retry loop always returns")
    }

    fn writer(
        &self,
        thread_id: &str,
        path: PathBuf,
    ) -> Result<(u64, mpsc::SyncSender<WriteRequest>), StorageError> {
        let mut writers = self
            .writers
            .lock()
            .map_err(|_| StorageError::Io("thread writer registry was poisoned".into()))?;
        if let Some(writer) = writers.get(thread_id) {
            return Ok((writer.id, writer.sender.clone()));
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(WRITER_CHANNEL_CAPACITY);
        let worker_thread_id = thread_id.to_string();
        let join = thread::Builder::new()
            .name(format!("k-coder-jsonl-{thread_id}"))
            .spawn(move || run_writer(worker_thread_id, path, receiver))
            .map_err(|error| StorageError::Io(error.to_string()))?;
        writers.insert(
            thread_id.to_string(),
            WriterHandle {
                id,
                sender: sender.clone(),
                join,
            },
        );
        Ok((id, sender))
    }

    fn remove_if_current(&self, thread_id: &str, writer_id: u64) -> Result<(), StorageError> {
        let writer = {
            let mut writers = self
                .writers
                .lock()
                .map_err(|_| StorageError::Io("thread writer registry was poisoned".into()))?;
            if writers
                .get(thread_id)
                .is_some_and(|writer| writer.id == writer_id)
            {
                writers.remove(thread_id)
            } else {
                None
            }
        };
        if let Some(writer) = writer {
            drop(writer.sender);
            writer
                .join
                .join()
                .map_err(|_| StorageError::Io("thread writer panicked".into()))?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn active_writer_count(&self) -> usize {
        self.writers.lock().unwrap().len()
    }
}

impl Drop for ThreadWriters {
    fn drop(&mut self) {
        let Ok(writers) = self.writers.get_mut() else {
            return;
        };
        let writers = std::mem::take(writers);
        let mut joins = Vec::with_capacity(writers.len());
        for (_, writer) in writers {
            drop(writer.sender);
            joins.push(writer.join);
        }
        for join in joins {
            let _ = join.join();
        }
    }
}

impl WriteRequest {
    fn new(line: Vec<u8>) -> (Self, oneshot::Receiver<Result<(), StorageError>>) {
        let (acknowledged, received) = oneshot::channel();
        (Self { line, acknowledged }, received)
    }
}

fn run_writer(thread_id: String, path: PathBuf, receiver: mpsc::Receiver<WriteRequest>) {
    let mut file = match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => file,
        Err(error) => {
            fail_pending(
                receiver,
                format!("cannot open JSONL writer for thread {thread_id}: {error}"),
            );
            return;
        }
    };
    while let Ok(request) = receiver.recv() {
        let result = file
            .write_all(&request.line)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_data())
            .map_err(|error| StorageError::Io(error.to_string()));
        let failed = result.is_err();
        let _ = request.acknowledged.send(result);
        if failed {
            fail_pending(
                receiver,
                format!("JSONL writer for thread {thread_id} stopped after an I/O failure"),
            );
            return;
        }
    }
}

fn fail_pending(receiver: mpsc::Receiver<WriteRequest>, message: String) {
    while let Ok(request) = receiver.try_recv() {
        let _ = request
            .acknowledged
            .send(Err(StorageError::Io(message.clone())));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn durable_ack_preserves_thread_order_and_reuses_the_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("thread.jsonl");
        let writers = ThreadWriters::new();

        writers
            .append("thread", path.clone(), br#"{"event":1}"#.to_vec())
            .await
            .unwrap();
        writers
            .append("thread", path.clone(), br#"{"event":2}"#.to_vec())
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\"event\":1}\n{\"event\":2}\n"
        );
        assert_eq!(writers.active_writer_count(), 1);
    }

    #[tokio::test]
    async fn different_threads_use_independent_writers() {
        let directory = tempfile::tempdir().unwrap();
        let writers = ThreadWriters::new();
        let first = writers.append(
            "first",
            directory.path().join("first.jsonl"),
            b"first".to_vec(),
        );
        let second = writers.append(
            "second",
            directory.path().join("second.jsonl"),
            b"second".to_vec(),
        );

        let (first_result, second_result) = tokio::join!(first, second);
        first_result.unwrap();
        second_result.unwrap();
        assert_eq!(writers.active_writer_count(), 2);
    }

    #[tokio::test]
    async fn writer_start_failure_returns_without_hanging() {
        let directory = tempfile::tempdir().unwrap();
        let writers = ThreadWriters::new();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            writers.append(
                "thread",
                directory.path().join("missing").join("thread.jsonl"),
                b"event".to_vec(),
            ),
        )
        .await
        .expect("writer failure should be bounded");

        assert!(matches!(result, Err(StorageError::Io(_))));
    }
}
