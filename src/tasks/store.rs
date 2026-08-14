use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use rusqlite::{Connection, OptionalExtension, params};
use tokio::{fs, sync::RwLock};

use crate::{
    config::Config,
    domain::task::TaskRecord,
    error::{GatewayError, Result},
};

pub(super) struct TaskStore {
    config: Arc<Config>,
    records: RwLock<HashMap<String, TaskRecord>>,
    database: Arc<StdMutex<Connection>>,
}

impl TaskStore {
    pub(super) async fn new(config: Arc<Config>) -> Result<Self> {
        fs::create_dir_all(&config.data_root).await?;
        let path = config.data_root.join("tasks.sqlite3");
        let connection = tokio::task::spawn_blocking(move || -> Result<Connection> {
            let connection = Connection::open(path).map_err(sql_error)?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(sql_error)?;
            connection
                .execute_batch(
                    "PRAGMA journal_mode=WAL;
                     PRAGMA synchronous=NORMAL;
                     PRAGMA foreign_keys=ON;
                     CREATE TABLE IF NOT EXISTS tasks (
                         id TEXT PRIMARY KEY,
                         status TEXT NOT NULL,
                         kind TEXT NOT NULL,
                         account_id TEXT,
                         created_at INTEGER NOT NULL,
                         updated_at INTEGER NOT NULL,
                         record_json BLOB NOT NULL
                     );
                     CREATE INDEX IF NOT EXISTS tasks_created_idx ON tasks(created_at DESC, id DESC);
                     CREATE INDEX IF NOT EXISTS tasks_status_idx ON tasks(status, created_at DESC);",
                )
                .map_err(sql_error)?;
            Ok(connection)
        })
        .await
        .map_err(join_error)??;
        let store = Self {
            config,
            records: RwLock::new(HashMap::new()),
            database: Arc::new(StdMutex::new(connection)),
        };
        store.migrate_json_tasks().await?;
        Ok(store)
    }

    pub(super) async fn insert(&self, record: TaskRecord) {
        self.records.write().await.insert(record.id.clone(), record);
    }

    pub(super) async fn remove_cached(&self, id: &str) {
        self.records.write().await.remove(id);
    }

    pub(super) async fn get(&self, id: &str) -> Result<Option<TaskRecord>> {
        if let Some(record) = self.records.read().await.get(id).cloned() {
            return Ok(Some(record));
        }
        let id = id.to_owned();
        let database = self.database.clone();
        let record = tokio::task::spawn_blocking(move || -> Result<Option<TaskRecord>> {
            let connection = database.lock().map_err(|_| {
                GatewayError::Internal(anyhow::anyhow!("task database lock poisoned"))
            })?;
            let bytes = connection
                .query_row(
                    "SELECT record_json FROM tasks WHERE id = ?1",
                    params![id],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(sql_error)?;
            bytes
                .map(|value| serde_json::from_slice(&value).map_err(Into::into))
                .transpose()
        })
        .await
        .map_err(join_error)??;
        if let Some(record) = &record {
            self.records
                .write()
                .await
                .insert(record.id.clone(), record.clone());
        }
        Ok(record)
    }

    pub(super) async fn update<F>(&self, id: &str, f: F) -> Result<TaskRecord>
    where
        F: FnOnce(&mut TaskRecord),
    {
        self.ensure_loaded(id).await?;
        let mut records = self.records.write().await;
        let record = records.get_mut(id).ok_or(GatewayError::TaskNotFound)?;
        f(record);
        Ok(record.clone())
    }

    pub(super) async fn transition<F>(&self, id: &str, f: F) -> Result<TaskRecord>
    where
        F: FnOnce(&mut TaskRecord) -> bool,
    {
        self.ensure_loaded(id).await?;
        let mut records = self.records.write().await;
        let record = records.get_mut(id).ok_or(GatewayError::TaskNotFound)?;
        let _ = f(record);
        Ok(record.clone())
    }

    async fn ensure_loaded(&self, id: &str) -> Result<()> {
        if self.records.read().await.contains_key(id) {
            return Ok(());
        }
        if self.get(id).await?.is_none() {
            return Err(GatewayError::TaskNotFound);
        }
        Ok(())
    }

    pub(super) async fn persist(&self, record: &TaskRecord) -> Result<()> {
        let record = record.clone();
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let bytes = serde_json::to_vec(&record)?;
            let connection = database.lock().map_err(|_| {
                GatewayError::Internal(anyhow::anyhow!("task database lock poisoned"))
            })?;
            connection
                .execute(
                    "INSERT INTO tasks(id,status,kind,account_id,created_at,updated_at,record_json)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(id) DO UPDATE SET
                       status=excluded.status,
                       kind=excluded.kind,
                       account_id=excluded.account_id,
                       updated_at=excluded.updated_at,
                       record_json=excluded.record_json",
                    params![
                        record.id,
                        record.status,
                        record.kind.as_str(),
                        record.account_id,
                        record.created_at as i64,
                        record.updated_at as i64,
                        bytes,
                    ],
                )
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)??;
        Ok(())
    }

    pub(super) async fn recover(&self) -> Result<Vec<String>> {
        let database = self.database.clone();
        let records = tokio::task::spawn_blocking(move || -> Result<Vec<TaskRecord>> {
            let connection = database
                .lock()
                .map_err(|_| GatewayError::Internal(anyhow::anyhow!("task database lock poisoned")))?;
            let mut statement = connection
                .prepare("SELECT record_json FROM tasks WHERE status IN ('queued','running') ORDER BY created_at, id")
                .map_err(sql_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(sql_error)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(sql_error)?;
                records.push(serde_json::from_slice(&bytes)?);
            }
            Ok(records)
        })
        .await
        .map_err(join_error)??;

        let mut queued = Vec::new();
        for mut record in records {
            if record.status == "running" {
                record.status = "interrupted".into();
                record.error = Some("Service restarted while the task was running".into());
                record.payload = None;
                record.updated_at = crate::util::time::epoch_seconds();
                self.persist(&record).await?;
            } else if record.status == "queued" {
                queued.push((record.created_at, record.id.clone()));
            }
            self.records.write().await.insert(record.id.clone(), record);
        }
        queued.sort_by_key(|item| item.0);
        Ok(queued.into_iter().map(|(_, id)| id).collect())
    }

    pub(super) async fn list(
        &self,
        limit: usize,
        before: Option<u64>,
        status: Option<String>,
    ) -> Result<Vec<TaskRecord>> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<TaskRecord>> {
            let connection = database.lock().map_err(|_| {
                GatewayError::Internal(anyhow::anyhow!("task database lock poisoned"))
            })?;
            let before = before.unwrap_or(u64::MAX).min(i64::MAX as u64) as i64;
            let mut statement = connection
                .prepare(
                    "SELECT record_json FROM tasks
                     WHERE created_at < ?1 AND (?2 IS NULL OR status = ?2)
                     ORDER BY created_at DESC, id DESC LIMIT ?3",
                )
                .map_err(sql_error)?;
            let rows = statement
                .query_map(params![before, status, limit as i64], |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .map_err(sql_error)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(serde_json::from_slice(&row.map_err(sql_error)?)?);
            }
            Ok(records)
        })
        .await
        .map_err(join_error)?
    }

    pub(super) async fn cleanup_expired(&self, cutoff: u64) -> Result<Vec<String>> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let mut connection = database
                .lock()
                .map_err(|_| GatewayError::Internal(anyhow::anyhow!("task database lock poisoned")))?;
            let transaction = connection.transaction().map_err(sql_error)?;
            let ids = {
                let mut statement = transaction
                    .prepare(
                        "SELECT id FROM tasks WHERE updated_at < ?1 AND status IN ('completed','failed','interrupted','rejected','canceled')",
                    )
                    .map_err(sql_error)?;
                let rows = statement
                    .query_map(params![cutoff.min(i64::MAX as u64) as i64], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(sql_error)?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(sql_error)?
            };
            transaction
                .execute(
                    "DELETE FROM tasks WHERE updated_at < ?1 AND status IN ('completed','failed','interrupted','rejected','canceled')",
                    params![cutoff.min(i64::MAX as u64) as i64],
                )
                .map_err(sql_error)?;
            transaction.commit().map_err(sql_error)?;
            Ok(ids)
        })
        .await
        .map_err(join_error)?
    }

    async fn migrate_json_tasks(&self) -> Result<()> {
        let legacy_root = self.config.data_root.join("tasks");
        if !legacy_root.is_dir() {
            return Ok(());
        }
        let database = self.database.clone();
        let is_empty = tokio::task::spawn_blocking(move || -> Result<bool> {
            let connection = database.lock().map_err(|_| {
                GatewayError::Internal(anyhow::anyhow!("task database lock poisoned"))
            })?;
            let count: i64 = connection
                .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
                .map_err(sql_error)?;
            Ok(count == 0)
        })
        .await
        .map_err(join_error)??;
        if !is_empty {
            return Ok(());
        }
        let mut entries = fs::read_dir(&legacy_root).await?;
        let mut migrated = 0usize;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path()).await else {
                continue;
            };
            let Ok(record) = serde_json::from_slice::<TaskRecord>(&bytes) else {
                continue;
            };
            self.persist(&record).await?;
            migrated += 1;
        }
        if migrated > 0 {
            tracing::info!(migrated, "migrated legacy JSON tasks into SQLite");
        }
        Ok(())
    }
}

fn sql_error(error: rusqlite::Error) -> GatewayError {
    GatewayError::Internal(anyhow::Error::new(error))
}

fn join_error(error: tokio::task::JoinError) -> GatewayError {
    GatewayError::Internal(anyhow::Error::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generation::GenerationKind;

    fn config(root: &std::path::Path) -> Arc<Config> {
        Arc::new(Config {
            project_root: root.into(),
            host: "127.0.0.1".into(),
            port: 8000,
            api_key: "test-key-1234".into(),
            admin_token: "admin-key-1234".into(),
            secure_1psid: "cookie-value-1234".into(),
            secure_1psidts: None,
            auth_user: 0,
            accounts_file: root.join("accounts.local.json"),
            account_cooldown: std::time::Duration::from_secs(60),
            account_probe_interval: std::time::Duration::from_secs(60),
            proxy: None,
            provider_origin: "https://gemini.google.com".into(),
            retry_attempts: 3,
            retry_delay: std::time::Duration::from_millis(1),
            cookie_sync_enabled: true,
            remote_admin_enabled: false,
            default_model: "gemini-3-flash".into(),
            advanced_model: "gemini-3-pro".into(),
            enabled_generations: vec!["image".into()],
            data_root: root.join("data"),
            public_base_url: "http://127.0.0.1:8000".into(),
            max_concurrency: 1,
            max_account_concurrency: 1,
            task_workers: 1,
            max_task_queue: 20,
            task_retention: std::time::Duration::from_secs(3600),
            queue_timeout: std::time::Duration::from_secs(1),
            provider_timeout: std::time::Duration::from_secs(1),
            max_messages: 50,
            max_prompt_chars: 200_000,
            max_request_bytes: 1_048_576,
            cors_origins: vec![],
        })
    }

    fn record() -> TaskRecord {
        TaskRecord {
            id: "0123456789abcdef0123456789abcdef".into(),
            object: "gemini.task".into(),
            kind: GenerationKind::Image,
            status: "completed".into(),
            created_at: 10,
            updated_at: 20,
            account_id: Some("default".into()),
            payload: None,
            progress: None,
            result: Some(serde_json::json!({"ok":true})),
            error: None,
        }
    }

    #[tokio::test]
    async fn sqlite_persists_lists_and_cleans_tasks() {
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        let store = TaskStore::new(config.clone()).await.unwrap();
        let record = record();
        store.persist(&record).await.unwrap();
        drop(store);

        let reopened = TaskStore::new(config).await.unwrap();
        assert_eq!(
            reopened.get(&record.id).await.unwrap().unwrap().account_id,
            Some("default".into())
        );
        assert_eq!(reopened.list(20, None, None).await.unwrap().len(), 1);
        assert_eq!(reopened.cleanup_expired(21).await.unwrap(), vec![record.id]);
    }
}
