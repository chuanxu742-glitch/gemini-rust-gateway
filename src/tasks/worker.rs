use std::{sync::Arc, time::Duration};

use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::{
    domain::task::TERMINAL_STATES,
    error::{GatewayError, Result},
    util::time::epoch_seconds,
};

use super::{TaskQueue, store::TaskStore};

pub(super) async fn run_worker(queue: Arc<TaskQueue>, index: usize) {
    loop {
        let task_id = tokio::select! {
            _ = queue.shutdown_token.cancelled() => break,
            value = async {
                queue.receiver.lock().await.recv().await
            } => match value {
                Some(value) => value,
                None => break,
            }
        };
        if let Err(error) = run_task(&queue, &task_id).await {
            tracing::warn!(worker = index, task_id, %error, "generation task failed");
        }
    }
}

async fn run_task(queue: &TaskQueue, task_id: &str) -> Result<()> {
    let (record, request) = {
        let mut payload = None;
        let record = queue
            .store
            .transition(task_id, |record| {
                if record.status != "queued" {
                    return false;
                }
                payload = record.payload.clone();
                record.status = "running".into();
                record.updated_at = epoch_seconds();
                true
            })
            .await?;
        if record.status != "running" {
            return Ok(());
        }
        let request = payload
            .ok_or_else(|| GatewayError::InvalidRequest("task payload is missing".into()))?;
        (record, request)
    };
    queue.store.persist(&record).await?;

    let cancellation = CancellationToken::new();
    queue
        .cancellations
        .write()
        .await
        .insert(task_id.to_owned(), cancellation.clone());
    let timeout_duration = Duration::from_secs_f64(request.timeout.clamp(1.0, 3600.0));
    let execution = queue.executor.execute(task_id, &request);
    let outcome = tokio::select! {
        _ = cancellation.cancelled() => Err(GatewayError::InvalidRequest("task canceled".into())),
        result = timeout(timeout_duration, execution) => {
            result.map_err(|_| GatewayError::Timeout).and_then(|result| result)
        }
    };
    queue.cancellations.write().await.remove(task_id);

    let final_record = queue
        .store
        .update(task_id, |record| {
            if record.status == "canceled" {
                return;
            }
            match outcome {
                Ok(result) => {
                    record.status = "completed".into();
                    record.result = Some(result);
                    record.error = None;
                }
                Err(error) => {
                    record.status = "failed".into();
                    record.error = Some(error.to_string());
                }
            }
            record.payload = None;
            record.progress = None;
            record.updated_at = epoch_seconds();
        })
        .await?;
    queue.store.persist(&final_record).await?;
    queue.notify(task_id).await;
    Ok(())
}

pub(super) fn is_terminal(status: &str) -> bool {
    TERMINAL_STATES.contains(&status)
}

#[allow(dead_code)]
fn _store_marker(_: &TaskStore) {}
