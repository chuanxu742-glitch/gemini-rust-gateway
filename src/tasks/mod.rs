pub mod executor;
pub mod store;
pub mod worker;

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{
    fs,
    sync::{Mutex, Notify, RwLock, mpsc},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    artifacts::ArtifactStore,
    config::Config,
    domain::{
        generation::GenerationRequest,
        task::{PublicTaskRecord, TaskRecord},
    },
    error::{GatewayError, Result},
    gemini::GeminiClient,
    util::time::epoch_seconds,
};

pub struct TaskQueue {
    config: Arc<Config>,
    #[allow(dead_code)]
    gemini: Arc<GeminiClient>,
    artifacts: ArtifactStore,
    executor: executor::TaskExecutor,
    store: store::TaskStore,
    sender: mpsc::Sender<String>,
    receiver: Mutex<mpsc::Receiver<String>>,
    events: RwLock<HashMap<String, Arc<Notify>>>,
    cancellations: RwLock<HashMap<String, CancellationToken>>,
    pub(super) shutdown_token: CancellationToken,
    workers: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl TaskQueue {
    pub async fn start(config: Arc<Config>, gemini: Arc<GeminiClient>) -> Result<Arc<Self>> {
        let task_dir = config.data_root.join("tasks");
        fs::create_dir_all(&task_dir).await?;
        let artifacts = ArtifactStore::new(
            config.data_root.join("artifacts"),
            config.public_base_url.clone(),
        )
        .await?;
        let store = store::TaskStore::new(config.clone()).await?;
        let executor =
            executor::TaskExecutor::new(config.clone(), gemini.clone(), artifacts.clone());
        let (sender, receiver) = mpsc::channel(config.max_task_queue);
        let queue = Arc::new(Self {
            config,
            gemini,
            artifacts,
            executor,
            store,
            sender,
            receiver: Mutex::new(receiver),
            events: RwLock::new(HashMap::new()),
            cancellations: RwLock::new(HashMap::new()),
            shutdown_token: CancellationToken::new(),
            workers: Mutex::new(Vec::new()),
        });
        let cutoff = epoch_seconds().saturating_sub(queue.config.task_retention.as_secs());
        for task_id in queue.store.cleanup_expired(cutoff).await? {
            if let Err(error) = queue.artifacts.remove_task(&task_id).await {
                tracing::warn!(task_id, %error, "failed to remove expired task artifacts");
            }
        }
        let queued = queue.store.recover().await?;
        for task_id in queued {
            if queue.sender.try_send(task_id.clone()).is_err() {
                let rejected = queue
                    .store
                    .update(&task_id, |record| {
                        record.status = "rejected".into();
                        record.error =
                            Some("Queue capacity exceeded during service recovery".into());
                        record.payload = None;
                        record.updated_at = epoch_seconds();
                    })
                    .await?;
                queue.store.persist(&rejected).await?;
            }
        }
        let mut handles = queue.workers.lock().await;
        for index in 0..queue.config.task_workers {
            let worker_queue = queue.clone();
            handles.push(tokio::spawn(async move {
                worker::run_worker(worker_queue, index).await;
            }));
        }
        let maintenance_queue = queue.clone();
        handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = maintenance_queue.shutdown_token.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(error) = maintenance_queue.cleanup_expired().await {
                            tracing::warn!(%error, "periodic task cleanup failed");
                        }
                    }
                }
            }
        }));
        drop(handles);
        Ok(queue)
    }

    pub fn artifacts_root(&self) -> &std::path::Path {
        self.artifacts.root()
    }

    pub async fn submit(&self, request: GenerationRequest) -> Result<PublicTaskRecord> {
        let now = epoch_seconds();
        let record = TaskRecord {
            id: Uuid::new_v4().simple().to_string(),
            object: "gemini.task".into(),
            kind: request.kind.clone(),
            status: "queued".into(),
            created_at: now,
            updated_at: now,
            account_id: request.account_id.clone(),
            payload: Some(request),
            progress: None,
            result: None,
            error: None,
        };
        self.events
            .write()
            .await
            .insert(record.id.clone(), Arc::new(Notify::new()));
        self.store.insert(record.clone()).await;
        self.store.persist(&record).await?;
        if self.sender.try_send(record.id.clone()).is_err() {
            let rejected = self
                .store
                .update(&record.id, |stored| {
                    stored.status = "rejected".into();
                    stored.error = Some("Generation task queue is full".into());
                    stored.payload = None;
                    stored.updated_at = epoch_seconds();
                })
                .await?;
            self.store.persist(&rejected).await?;
            self.notify(&record.id).await;
            return Err(GatewayError::QueueFull);
        }
        Ok(record.public(Some(&self.config.public_base_url)))
    }

    pub async fn get(&self, task_id: &str) -> Result<PublicTaskRecord> {
        validate_task_id(task_id)?;
        if let Some(record) = self.store.get(task_id).await? {
            return Ok(record.public(Some(&self.config.public_base_url)));
        }
        Err(GatewayError::TaskNotFound)
    }

    pub async fn list(
        &self,
        limit: usize,
        before: Option<u64>,
        status: Option<String>,
    ) -> Result<Vec<PublicTaskRecord>> {
        let records = self.store.list(limit.clamp(1, 100), before, status).await?;
        Ok(records
            .into_iter()
            .map(|record| record.public(Some(&self.config.public_base_url)))
            .collect())
    }

    pub async fn cancel(&self, task_id: &str) -> Result<PublicTaskRecord> {
        validate_task_id(task_id)?;
        let record = self
            .store
            .update(task_id, |record| {
                if !record.terminal() {
                    record.status = "canceled".into();
                    record.error = Some("Task canceled by user".into());
                    record.progress = None;
                    record.payload = None;
                    record.updated_at = epoch_seconds();
                }
            })
            .await?;
        if let Some(token) = self.cancellations.read().await.get(task_id) {
            token.cancel();
        }
        self.store.persist(&record).await?;
        self.notify(task_id).await;
        Ok(record.public(Some(&self.config.public_base_url)))
    }

    pub async fn wait(&self, task_id: &str, timeout: Duration) -> Result<PublicTaskRecord> {
        validate_task_id(task_id)?;
        let event = {
            let mut events = self.events.write().await;
            events
                .entry(task_id.to_owned())
                .or_insert_with(|| Arc::new(Notify::new()))
                .clone()
        };
        let future = async {
            loop {
                let notified = event.notified();
                let record = self.get(task_id).await?;
                if worker::is_terminal(&record.status) {
                    return Ok(record);
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| GatewayError::Timeout)?
    }

    pub async fn shutdown(&self) {
        self.shutdown_token.cancel();
        for cancellation in self.cancellations.read().await.values() {
            cancellation.cancel();
        }
        let handles = std::mem::take(&mut *self.workers.lock().await);
        for handle in handles {
            let _ = handle.await;
        }
    }

    async fn cleanup_expired(&self) -> Result<()> {
        let cutoff = epoch_seconds().saturating_sub(self.config.task_retention.as_secs());
        for task_id in self.store.cleanup_expired(cutoff).await? {
            self.store.remove_cached(&task_id).await;
            if let Err(error) = self.artifacts.remove_task(&task_id).await {
                tracing::warn!(task_id, %error, "failed to remove expired task artifacts");
            }
        }
        Ok(())
    }

    pub(super) async fn notify(&self, task_id: &str) {
        if let Some(event) = self.events.read().await.get(task_id) {
            event.notify_waiters();
        }
    }
}

fn validate_task_id(task_id: &str) -> Result<()> {
    if task_id.len() != 32 || !task_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GatewayError::TaskNotFound);
    }
    Ok(())
}
