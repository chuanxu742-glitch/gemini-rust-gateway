pub mod accounts;
pub mod batch;
pub mod framing;
pub mod protocol;
pub mod session;
pub mod stream;

use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::{Stream, StreamExt};
use serde::Serialize;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    config::Config,
    domain::output::GeminiOutput,
    error::{GatewayError, Result},
};

use accounts::AccountCredentials;

#[derive(Debug, Clone)]
pub struct GeminiDelta {
    pub output: GeminiOutput,
    pub text_delta: String,
    pub account_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountStatus {
    pub id: String,
    pub auth_user: usize,
    pub enabled: bool,
    pub weight: usize,
    pub ready: bool,
    pub inflight: usize,
    pub cooldown_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub struct GeminiClient {
    config: Arc<Config>,
    accounts: RwLock<HashMap<String, Arc<AccountRuntime>>>,
    order: RwLock<Vec<String>>,
    persistence_lock: Mutex<()>,
    reqid: AtomicU64,
    cursor: AtomicUsize,
    global_slots: Arc<Semaphore>,
    ready_count: AtomicUsize,
    shutdown: CancellationToken,
    health_task: Mutex<Option<JoinHandle<()>>>,
}

struct AccountRuntime {
    id: String,
    state: RwLock<AccountState>,
    refresh_lock: Mutex<()>,
    slots: Arc<Semaphore>,
    enabled: AtomicBool,
    weight: AtomicUsize,
    ready: AtomicBool,
    inflight: AtomicUsize,
    cooldown_until_ms: AtomicU64,
    last_error: RwLock<Option<String>>,
}

#[derive(Clone)]
struct AccountState {
    http: reqwest::Client,
    jar: Arc<reqwest::cookie::Jar>,
    metadata: session::SessionMetadata,
    config: Arc<Config>,
}

struct RequestGuards {
    _global: OwnedSemaphorePermit,
    _account: OwnedSemaphorePermit,
    account: Arc<AccountRuntime>,
}

impl Drop for RequestGuards {
    fn drop(&mut self) {
        self.account.inflight.fetch_sub(1, Ordering::AcqRel);
    }
}

impl GeminiClient {
    pub async fn initialize(config: Arc<Config>) -> Result<Self> {
        let client = Self::degraded(config)?;
        let ids = client.order.read().await.clone();
        let mut last_error = None;
        for id in ids {
            let account = client.account(&id).await?;
            if !account.enabled.load(Ordering::Acquire) {
                continue;
            }
            if account.state.read().await.config.secure_1psid.is_empty() {
                continue;
            }
            match account.activate(&client.reqid).await {
                Ok(()) => {
                    client.set_ready(&account, true);
                    client.persist_rotated_cookie(&account).await;
                }
                Err(error) => {
                    account.set_error(error.to_string()).await;
                    last_error = Some(error);
                }
            }
        }
        if client.is_ready() {
            Ok(client)
        } else {
            Err(last_error.unwrap_or_else(|| {
                GatewayError::NotReady(
                    "no Gemini account is ready; sync a Cookie from the Chrome extension".into(),
                )
            }))
        }
    }

    pub fn degraded(config: Arc<Config>) -> Result<Self> {
        let credentials = accounts::load(&config)?;
        let mut runtimes = HashMap::new();
        let mut order = Vec::new();
        for credentials in credentials {
            let runtime = Arc::new(AccountRuntime::degraded(&config, credentials)?);
            order.push(runtime.id.clone());
            runtimes.insert(runtime.id.clone(), runtime);
        }
        order.sort();
        Ok(Self {
            config: config.clone(),
            accounts: RwLock::new(runtimes),
            order: RwLock::new(order),
            persistence_lock: Mutex::new(()),
            reqid: AtomicU64::new(rand::random_range(10_000..100_000)),
            cursor: AtomicUsize::new(0),
            global_slots: Arc::new(Semaphore::new(config.max_concurrency)),
            ready_count: AtomicUsize::new(0),
            shutdown: CancellationToken::new(),
            health_task: Mutex::new(None),
        })
    }

    pub fn is_ready(&self) -> bool {
        self.ready_count.load(Ordering::Acquire) > 0
    }

    pub async fn start_health_monitor(self: &Arc<Self>) {
        let mut task = self.health_task.lock().await;
        if task.is_some() {
            return;
        }
        let client = self.clone();
        *task = Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(client.config.account_probe_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = client.shutdown.cancelled() => break,
                    _ = interval.tick() => client.recover_unready_accounts().await,
                }
            }
        }));
    }

    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        if let Some(task) = self.health_task.lock().await.take() {
            let _ = task.await;
        }
    }

    async fn recover_unready_accounts(&self) {
        let accounts = self
            .accounts
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for account in accounts {
            if !account.enabled.load(Ordering::Acquire)
                || account.ready.load(Ordering::Acquire)
                || account.cooldown_until_ms.load(Ordering::Acquire) > now_millis()
                || account.state.read().await.config.secure_1psid.is_empty()
            {
                continue;
            }
            match account.activate(&self.reqid).await {
                Ok(()) => {
                    self.set_ready(&account, true);
                    self.persist_rotated_cookie(&account).await;
                    tracing::info!(account_id = %account.id, "Gemini account recovered");
                }
                Err(error) => {
                    account
                        .cooldown(error.to_string(), self.config.account_probe_interval)
                        .await;
                    tracing::warn!(account_id = %account.id, %error, "Gemini account recovery probe failed");
                }
            }
        }
    }

    pub async fn resolve_account(&self, requested: Option<&str>) -> Result<String> {
        Ok(self.select_account(requested).await?.id.clone())
    }

    pub async fn generate(
        &self,
        prompt: &str,
        model: &str,
        feature_mode: i64,
    ) -> Result<GeminiOutput> {
        self.generate_for(None, prompt, model, feature_mode).await
    }

    pub async fn generate_for(
        &self,
        requested: Option<&str>,
        prompt: &str,
        model: &str,
        feature_mode: i64,
    ) -> Result<GeminiOutput> {
        let mut last_error = None;
        for attempt in 0..self.config.retry_attempts {
            match self
                .generate_stream_for(requested, prompt, model, feature_mode)
                .await
            {
                Ok(mut stream) => {
                    let mut output = GeminiOutput::default();
                    let mut failed = None;
                    while let Some(update) = stream.next().await {
                        match update {
                            Ok(update) => output = update.output,
                            Err(error) => {
                                failed = Some(error);
                                break;
                            }
                        }
                    }
                    if let Some(error) = failed {
                        if !error.is_retryable() {
                            return Err(error);
                        }
                        last_error = Some(error);
                    } else {
                        return Ok(output);
                    }
                }
                Err(error) => {
                    if !error.is_retryable() {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
            if attempt + 1 < self.config.retry_attempts {
                self.retry_delay(attempt).await;
            }
        }
        Err(last_error.unwrap_or_else(|| GatewayError::Provider("generation failed".into())))
    }

    pub async fn generate_stream(
        &self,
        prompt: &str,
        model: &str,
        feature_mode: i64,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<GeminiDelta>> + Send + 'static>>> {
        self.generate_stream_for(None, prompt, model, feature_mode)
            .await
    }

    pub async fn generate_stream_for(
        &self,
        requested: Option<&str>,
        prompt: &str,
        model: &str,
        feature_mode: i64,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<GeminiDelta>> + Send + 'static>>> {
        if prompt.is_empty() {
            return Err(GatewayError::InvalidRequest(
                "prompt cannot be empty".into(),
            ));
        }
        let mut last_error = None;
        for attempt in 0..self.config.retry_attempts {
            let account = match self.select_account(requested).await {
                Ok(account) => account,
                Err(_error) if last_error.is_some() => break,
                Err(error) => return Err(error),
            };
            let guards = self.acquire(&account).await?;
            let state = account.state.read().await.clone();
            let request_id = self.reqid.fetch_add(100_000, Ordering::Relaxed);
            let ctx = stream::build_stream_context(
                prompt,
                model,
                feature_mode,
                &state.metadata,
                request_id,
                state.config.auth_user,
                &state.config.provider_origin,
            )?;
            let response = match stream::build_request(&state.http, &ctx, model).send().await {
                Ok(response) => response,
                Err(error) => {
                    let message = format!("request failed: {error}");
                    account
                        .cooldown(message.clone(), Duration::from_secs(5))
                        .await;
                    last_error = Some(message);
                    drop(guards);
                    if attempt + 1 < self.config.retry_attempts {
                        self.retry_delay(attempt).await;
                    }
                    continue;
                }
            };
            let status = response.status();
            if status.is_success() {
                account.clear_error().await;
                // The response headers are in; if this request carried a
                // rotated cookie the jar already holds it.
                self.persist_rotated_cookie(&account).await;
                let account_id = account.id.clone();
                let monitored = account.clone();
                let mut provider = stream::parse_response_stream(
                    response.bytes_stream(),
                    self.config.provider_timeout,
                    guards,
                );
                let output = async_stream::stream! {
                    while let Some(update) = provider.next().await {
                        match update {
                            Ok(mut update) => {
                                update.account_id = account_id.clone();
                                yield Ok(update);
                            }
                            Err(error) => {
                                monitored.cooldown(error.to_string(), Duration::from_secs(5)).await;
                                yield Err(error);
                                break;
                            }
                        }
                    }
                };
                return Ok(Box::pin(output));
            }

            let message = format!("StreamGenerate returned HTTP {status}");
            last_error = Some(message.clone());
            match status.as_u16() {
                400 | 405 => {
                    if let Err(error) = self.refresh_account(&account).await {
                        account.set_error(error.to_string()).await;
                    }
                }
                401 | 403 => {
                    account
                        .cooldown(message, self.config.account_probe_interval)
                        .await;
                    self.set_ready(&account, false);
                }
                429 => {
                    account
                        .cooldown(message, self.config.account_cooldown)
                        .await;
                }
                _ if status.is_server_error() || matches!(status.as_u16(), 408 | 425) => {
                    account.cooldown(message, Duration::from_secs(5)).await;
                }
                _ => {
                    drop(guards);
                    return Err(GatewayError::Provider(format!(
                        "StreamGenerate returned HTTP {status} for account {}",
                        account.id
                    )));
                }
            }
            drop(guards);
            if attempt + 1 < self.config.retry_attempts {
                self.retry_delay(attempt).await;
            }
        }
        Err(GatewayError::Provider(format!(
            "all eligible Gemini accounts failed{}",
            last_error
                .map(|value| format!(": {value}"))
                .unwrap_or_default()
        )))
    }

    async fn acquire(&self, account: &Arc<AccountRuntime>) -> Result<RequestGuards> {
        let global = tokio::time::timeout(
            self.config.queue_timeout,
            self.global_slots.clone().acquire_owned(),
        )
        .await
        .map_err(|_| GatewayError::QueueFull)?
        .map_err(|_| GatewayError::Provider("global provider semaphore was closed".into()))?;
        let account_permit = tokio::time::timeout(
            self.config.queue_timeout,
            account.slots.clone().acquire_owned(),
        )
        .await
        .map_err(|_| GatewayError::QueueFull)?
        .map_err(|_| GatewayError::Provider("account provider semaphore was closed".into()))?;
        account.inflight.fetch_add(1, Ordering::AcqRel);
        Ok(RequestGuards {
            _global: global,
            _account: account_permit,
            account: account.clone(),
        })
    }

    async fn select_account(&self, requested: Option<&str>) -> Result<Arc<AccountRuntime>> {
        if let Some(id) = requested {
            accounts::validate_account_id(id)?;
            let account = self.account(id).await?;
            self.ensure_available(&account)?;
            return Ok(account);
        }

        let order = self.order.read().await.clone();
        let accounts = self.accounts.read().await;
        let mut eligible = order
            .iter()
            .filter_map(|id| accounts.get(id).cloned())
            .filter(|account| self.is_available(account))
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            return Err(GatewayError::NotReady(
                "no healthy Gemini account is available; inspect /v1/accounts or sync cookies"
                    .into(),
            ));
        }
        let minimum = eligible.iter().map(account_load_score).min().unwrap_or(0);
        eligible.retain(|account| account_load_score(account) == minimum);
        let total_weight = eligible
            .iter()
            .map(|account| account.weight.load(Ordering::Acquire).max(1))
            .sum::<usize>();
        let mut slot = self.cursor.fetch_add(1, Ordering::Relaxed) % total_weight;
        for account in &eligible {
            let weight = account.weight.load(Ordering::Acquire).max(1);
            if slot < weight {
                return Ok(account.clone());
            }
            slot -= weight;
        }
        Ok(eligible.swap_remove(0))
    }

    fn is_available(&self, account: &AccountRuntime) -> bool {
        account.enabled.load(Ordering::Acquire)
            && account.ready.load(Ordering::Acquire)
            && account.cooldown_until_ms.load(Ordering::Acquire) <= now_millis()
    }

    fn ensure_available(&self, account: &AccountRuntime) -> Result<()> {
        if !account.enabled.load(Ordering::Acquire) {
            return Err(GatewayError::NotReady(format!(
                "Gemini account '{}' is disabled",
                account.id
            )));
        }
        if !account.ready.load(Ordering::Acquire) {
            return Err(GatewayError::NotReady(format!(
                "Gemini account '{}' is not ready; sync its cookies",
                account.id
            )));
        }
        let remaining = account
            .cooldown_until_ms
            .load(Ordering::Acquire)
            .saturating_sub(now_millis())
            .div_ceil(1000);
        if remaining > 0 {
            return Err(GatewayError::NotReady(format!(
                "Gemini account '{}' is cooling down for {remaining}s",
                account.id
            )));
        }
        Ok(())
    }

    async fn account(&self, id: &str) -> Result<Arc<AccountRuntime>> {
        self.accounts
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| GatewayError::InvalidRequest(format!("unknown Gemini account: {id}")))
    }

    async fn refresh_account(&self, account: &Arc<AccountRuntime>) -> Result<()> {
        {
            let _guard = account.refresh_lock.lock().await;
            let state = account.state.read().await.clone();
            let metadata = session::fetch_session(&state.http, &state.jar, &state.config).await?;
            account.state.write().await.metadata = metadata;
            account.clear_error().await;
        }
        self.set_ready(account, true);
        self.persist_rotated_cookie(account).await;
        Ok(())
    }

    /// Move a rotated cookie from the jar into the account config and write it
    /// to disk, so the rotation survives a restart.
    ///
    /// Persistence failures are logged rather than propagated: the request that
    /// carried the rotation already succeeded, and the absorbed value stays
    /// usable in memory until the next one arrives.
    async fn persist_rotated_cookie(&self, account: &Arc<AccountRuntime>) {
        if !account.absorb_rotated_cookie().await {
            return;
        }
        match self.persist_accounts().await {
            Ok(()) => {
                tracing::debug!(account_id = %account.id, "persisted rotated Gemini cookie");
            }
            Err(error) => {
                tracing::warn!(
                    account_id = %account.id,
                    %error,
                    "could not persist rotated Gemini cookie"
                );
            }
        }
    }

    async fn persist_accounts(&self) -> Result<()> {
        let _guard = self.persistence_lock.lock().await;
        let mut stored = Vec::new();
        for account in self.accounts.read().await.values() {
            stored.push(account.credentials().await);
        }
        stored.sort_by(|left, right| left.id.cmp(&right.id));
        accounts::persist(&self.config.accounts_file, &stored).await
    }

    async fn retry_delay(&self, attempt: usize) {
        let multiplier = 1_u32 << attempt.min(5);
        let jitter = rand::random_range(0..=250);
        tokio::time::sleep(
            self.config.retry_delay.saturating_mul(multiplier) + Duration::from_millis(jitter),
        )
        .await;
    }

    fn set_ready(&self, account: &AccountRuntime, value: bool) {
        let previous = account.ready.swap(value, Ordering::AcqRel);
        match (previous, value) {
            (false, true) => {
                self.ready_count.fetch_add(1, Ordering::AcqRel);
            }
            (true, false) => {
                self.ready_count.fetch_sub(1, Ordering::AcqRel);
            }
            _ => {}
        }
    }

    pub async fn upsert_credentials(
        &self,
        account_id: String,
        auth_user: usize,
        secure_1psid: String,
        secure_1psidts: Option<String>,
    ) -> Result<()> {
        accounts::validate_account_id(&account_id)?;
        let credentials = AccountCredentials {
            id: account_id.clone(),
            auth_user,
            secure_1psid,
            secure_1psidts,
            enabled: true,
            weight: 1,
        };
        let runtime = Arc::new(AccountRuntime::degraded(&self.config, credentials)?);
        runtime.activate(&self.reqid).await?;

        let _guard = self.persistence_lock.lock().await;
        let mut stored = Vec::new();
        for account in self.accounts.read().await.values() {
            if account.id != account_id {
                stored.push(account.credentials().await);
            }
        }
        stored.push(runtime.credentials().await);
        stored.sort_by(|left, right| left.id.cmp(&right.id));
        accounts::persist(&self.config.accounts_file, &stored).await?;

        let previous = self
            .accounts
            .write()
            .await
            .insert(account_id.clone(), runtime.clone());
        if previous
            .as_ref()
            .is_some_and(|account| account.ready.load(Ordering::Acquire))
        {
            self.ready_count.fetch_sub(1, Ordering::AcqRel);
        }
        self.set_ready(&runtime, true);
        let mut order = self.order.write().await;
        if !order.contains(&account_id) {
            order.push(account_id);
            order.sort();
        }
        Ok(())
    }

    pub async fn configure_account(
        &self,
        account_id: &str,
        enabled: Option<bool>,
        weight: Option<usize>,
    ) -> Result<AccountStatus> {
        accounts::validate_account_id(account_id)?;
        if weight.is_some_and(|value| !(1..=100).contains(&value)) {
            return Err(GatewayError::InvalidRequest(
                "account weight must be between 1 and 100".into(),
            ));
        }
        let _guard = self.persistence_lock.lock().await;
        let account = self.account(account_id).await?;
        if let Some(value) = enabled {
            account.enabled.store(value, Ordering::Release);
            if !value {
                self.set_ready(&account, false);
            } else if !account.ready.load(Ordering::Acquire) {
                account.activate(&self.reqid).await?;
                self.set_ready(&account, true);
            }
        }
        if let Some(value) = weight {
            account.weight.store(value, Ordering::Release);
        }
        self.persist_snapshot().await?;
        Ok(self.account_status(account).await)
    }

    pub async fn delete_account(&self, account_id: &str) -> Result<()> {
        accounts::validate_account_id(account_id)?;
        let _guard = self.persistence_lock.lock().await;
        let removed = self
            .accounts
            .write()
            .await
            .remove(account_id)
            .ok_or_else(|| {
                GatewayError::InvalidRequest(format!("unknown Gemini account: {account_id}"))
            })?;
        if removed.ready.load(Ordering::Acquire) {
            self.ready_count.fetch_sub(1, Ordering::AcqRel);
        }
        self.order.write().await.retain(|id| id != account_id);
        if let Err(error) = self.persist_snapshot().await {
            self.accounts
                .write()
                .await
                .insert(account_id.to_owned(), removed.clone());
            self.order.write().await.push(account_id.to_owned());
            self.order.write().await.sort();
            if removed.ready.load(Ordering::Acquire) {
                self.ready_count.fetch_add(1, Ordering::AcqRel);
            }
            return Err(error);
        }
        Ok(())
    }

    async fn persist_snapshot(&self) -> Result<()> {
        let accounts = self
            .accounts
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut stored = Vec::with_capacity(accounts.len());
        for account in accounts {
            stored.push(account.credentials().await);
        }
        stored.sort_by(|left, right| left.id.cmp(&right.id));
        accounts::persist(&self.config.accounts_file, &stored).await
    }

    pub async fn sync_credentials(
        &self,
        auth_user: usize,
        secure_1psid: String,
        secure_1psidts: Option<String>,
    ) -> Result<()> {
        self.upsert_credentials("default".into(), auth_user, secure_1psid, secure_1psidts)
            .await
    }

    pub async fn http_for(&self, account_id: &str) -> Result<reqwest::Client> {
        let account = self.account(account_id).await?;
        Ok(account.state.read().await.http.clone())
    }

    pub async fn account_statuses(&self) -> Vec<AccountStatus> {
        let order = self.order.read().await.clone();
        let accounts = self.accounts.read().await;
        let mut statuses = Vec::new();
        for id in order {
            let Some(account) = accounts.get(&id) else {
                continue;
            };
            let mut status = self.account_status(account.clone()).await;
            status.id = id;
            statuses.push(status);
        }
        statuses
    }

    async fn account_status(&self, account: Arc<AccountRuntime>) -> AccountStatus {
        let auth_user = account.state.read().await.config.auth_user;
        AccountStatus {
            id: account.id.clone(),
            auth_user,
            enabled: account.enabled.load(Ordering::Acquire),
            weight: account.weight.load(Ordering::Acquire),
            ready: account.ready.load(Ordering::Acquire),
            inflight: account.inflight.load(Ordering::Acquire),
            cooldown_seconds: account
                .cooldown_until_ms
                .load(Ordering::Acquire)
                .saturating_sub(now_millis())
                .div_ceil(1000),
            last_error: account.last_error.read().await.clone(),
        }
    }
}

impl AccountRuntime {
    fn degraded(config: &Arc<Config>, credentials: AccountCredentials) -> Result<Self> {
        let mut account_config = (**config).clone();
        account_config.auth_user = credentials.auth_user;
        account_config.secure_1psid = credentials.secure_1psid;
        account_config.secure_1psidts = credentials.secure_1psidts;
        let account_config = Arc::new(account_config);
        let (http, jar) = session::build_http_client(&account_config)?;
        Ok(Self {
            id: credentials.id,
            state: RwLock::new(AccountState {
                http,
                jar,
                metadata: session::SessionMetadata {
                    access_token: None,
                    build_label: None,
                    session_id: None,
                    language: "en".into(),
                },
                config: account_config,
            }),
            refresh_lock: Mutex::new(()),
            slots: Arc::new(Semaphore::new(config.max_account_concurrency)),
            enabled: AtomicBool::new(credentials.enabled),
            weight: AtomicUsize::new(credentials.weight),
            ready: AtomicBool::new(false),
            inflight: AtomicUsize::new(0),
            cooldown_until_ms: AtomicU64::new(0),
            last_error: RwLock::new(None),
        })
    }

    async fn activate(&self, reqid: &AtomicU64) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let state = self.state.read().await.clone();
        let metadata = session::fetch_session(&state.http, &state.jar, &state.config).await?;
        let request_id = reqid.fetch_add(100_000, Ordering::Relaxed);
        batch::batch_execute(
            &state.http,
            "ESY5D",
            r#"[[["bard_activity_enabled"]]]"#,
            &metadata,
            request_id,
            state.config.auth_user,
            &state.config.provider_origin,
        )
        .await?;
        self.state.write().await.metadata = metadata;
        self.clear_error().await;
        Ok(())
    }

    /// Pull a rotated `__Secure-1PSIDTS` out of the jar into the account
    /// config. Returns whether the value actually changed.
    ///
    /// All three outcomes are logged so the rotation cadence can be observed
    /// without guessing: an absent cookie means the provider never sent one,
    /// which is not the same as a rotation that failed to be absorbed. The
    /// value itself is never logged; its length is enough to tell rotations
    /// apart and is safe to leave in a shared log.
    async fn absorb_rotated_cookie(&self) -> bool {
        let state = self.state.read().await.clone();
        let had_previous = state.config.secure_1psidts.is_some();
        match session::psidts_state(&state.jar, state.config.secure_1psidts.as_deref()) {
            session::Psidts::Absent => {
                tracing::debug!(
                    account_id = %self.id,
                    "no __Secure-1PSIDTS in the cookie jar; the provider has not sent one"
                );
                false
            }
            session::Psidts::Unchanged => {
                tracing::debug!(
                    account_id = %self.id,
                    "__Secure-1PSIDTS unchanged since the last check"
                );
                false
            }
            session::Psidts::Rotated(value) => {
                tracing::info!(
                    account_id = %self.id,
                    length = value.len(),
                    had_previous,
                    "absorbed a rotated __Secure-1PSIDTS from the provider"
                );
                let mut config = (*state.config).clone();
                config.secure_1psidts = Some(value);
                self.state.write().await.config = Arc::new(config);
                true
            }
        }
    }

    async fn credentials(&self) -> AccountCredentials {
        let state = self.state.read().await;
        AccountCredentials {
            id: self.id.clone(),
            auth_user: state.config.auth_user,
            secure_1psid: state.config.secure_1psid.clone(),
            secure_1psidts: state.config.secure_1psidts.clone(),
            enabled: self.enabled.load(Ordering::Acquire),
            weight: self.weight.load(Ordering::Acquire),
        }
    }

    async fn set_error(&self, message: String) {
        *self.last_error.write().await = Some(message);
    }

    async fn clear_error(&self) {
        self.cooldown_until_ms.store(0, Ordering::Release);
        *self.last_error.write().await = None;
    }

    async fn cooldown(&self, message: String, duration: Duration) {
        self.cooldown_until_ms.store(
            now_millis().saturating_add(duration.as_millis() as u64),
            Ordering::Release,
        );
        self.set_error(message).await;
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn account_load_score(account: &Arc<AccountRuntime>) -> usize {
    account
        .inflight
        .load(Ordering::Acquire)
        .saturating_mul(1000)
        / account.weight.load(Ordering::Acquire).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::Path,
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
    };

    fn test_config(root: &std::path::Path) -> Arc<Config> {
        Arc::new(Config {
            project_root: root.into(),
            host: "127.0.0.1".into(),
            port: 8000,
            api_key: "test-key-1234".into(),
            admin_token: "admin-key-1234".into(),
            secure_1psid: "default-cookie-value".into(),
            secure_1psidts: None,
            auth_user: 0,
            accounts_file: root.join("accounts.local.json"),
            account_cooldown: Duration::from_secs(60),
            account_probe_interval: Duration::from_secs(60),
            proxy: None,
            provider_origin: "https://gemini.google.com".into(),
            retry_attempts: 3,
            retry_delay: Duration::from_millis(1),
            cookie_sync_enabled: true,
            remote_admin_enabled: false,
            default_model: "gemini-3-flash".into(),
            advanced_model: "gemini-3-pro".into(),
            enabled_generations: vec!["image".into()],
            data_root: root.join("data"),
            public_base_url: "http://127.0.0.1:8000".into(),
            max_concurrency: 4,
            max_account_concurrency: 1,
            task_workers: 1,
            max_task_queue: 20,
            task_retention: Duration::from_secs(7 * 24 * 3600),
            queue_timeout: Duration::from_secs(1),
            provider_timeout: Duration::from_secs(1),
            max_messages: 50,
            max_prompt_chars: 200_000,
            max_request_bytes: 1_048_576,
            cors_origins: vec![],
        })
    }

    async fn pool_with_two_accounts() -> (tempfile::TempDir, GeminiClient) {
        let root = tempfile::tempdir().unwrap();
        let config = test_config(root.path());
        let pool = GeminiClient::degraded(config.clone()).unwrap();
        let second = Arc::new(
            AccountRuntime::degraded(
                &config,
                AccountCredentials {
                    id: "second".into(),
                    auth_user: 1,
                    secure_1psid: "second-cookie-value".into(),
                    secure_1psidts: None,
                    enabled: true,
                    weight: 1,
                },
            )
            .unwrap(),
        );
        pool.accounts
            .write()
            .await
            .insert("second".into(), second.clone());
        *pool.order.write().await = vec!["default".into(), "second".into()];
        let default = pool.account("default").await.unwrap();
        pool.set_ready(&default, true);
        pool.set_ready(&second, true);
        (root, pool)
    }

    #[tokio::test]
    async fn unready_pool_fails_fast_without_burning_retries() {
        let root = tempfile::tempdir().unwrap();
        let mut config = (*test_config(root.path())).clone();
        // Large enough that a single wasted retry would be unmistakable.
        config.retry_delay = Duration::from_millis(500);
        let pool = GeminiClient::degraded(Arc::new(config)).unwrap();

        let started = std::time::Instant::now();
        let error = pool
            .generate("hello", "gemini-3-flash", 0)
            .await
            .expect_err("a degraded pool has no ready account");
        let elapsed = started.elapsed();

        assert!(
            matches!(error, GatewayError::NotReady(_)),
            "expected NotReady, got {error:?}"
        );
        assert!(
            elapsed < Duration::from_millis(250),
            "a known-unready pool retried instead of failing fast: {elapsed:?}"
        );
    }

    #[test]
    fn account_status_never_contains_credentials() {
        let status = AccountStatus {
            id: "work".into(),
            auth_user: 1,
            enabled: true,
            weight: 1,
            ready: true,
            inflight: 0,
            cooldown_seconds: 0,
            last_error: None,
        };
        let value = serde_json::to_string(&status).unwrap();
        assert!(!value.contains("secure_1psid"));
    }

    #[tokio::test]
    async fn rotates_equally_loaded_accounts() {
        let (_root, pool) = pool_with_two_accounts().await;
        let first = pool.resolve_account(None).await.unwrap();
        let second = pool.resolve_account(None).await.unwrap();
        assert_ne!(first, second);
        assert!(matches!(first.as_str(), "default" | "second"));
        assert!(matches!(second.as_str(), "default" | "second"));
    }

    #[tokio::test]
    async fn selects_least_inflight_and_skips_cooldown() {
        let (_root, pool) = pool_with_two_accounts().await;
        let default = pool.account("default").await.unwrap();
        let second = pool.account("second").await.unwrap();
        default.inflight.store(2, Ordering::Release);
        assert_eq!(pool.resolve_account(None).await.unwrap(), "second");
        second
            .cooldown("rate limited".into(), Duration::from_secs(60))
            .await;
        assert_eq!(pool.resolve_account(None).await.unwrap(), "default");
        assert!(pool.resolve_account(Some("second")).await.is_err());
    }

    #[tokio::test]
    async fn honors_account_weights_for_idle_rotation() {
        let (_root, pool) = pool_with_two_accounts().await;
        pool.account("second")
            .await
            .unwrap()
            .weight
            .store(3, Ordering::Release);
        let mut default_count = 0;
        let mut second_count = 0;
        for _ in 0..40 {
            match pool.resolve_account(None).await.unwrap().as_str() {
                "default" => default_count += 1,
                "second" => second_count += 1,
                _ => unreachable!(),
            }
        }
        assert_eq!(default_count, 10);
        assert_eq!(second_count, 30);
    }

    #[tokio::test]
    async fn mock_provider_fails_over_after_rate_limit() {
        async fn session_page() -> &'static str {
            r#"{"SNlM0e":"token","cfb2h":"build","FdrFJe":"session","TuX5cc":"en"}"#
        }
        async fn batch_ok() -> &'static str {
            "ok"
        }
        async fn generate(Path(auth_user): Path<usize>) -> axum::response::Response {
            if auth_user == 0 {
                return (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response();
            }
            let inner = include_str!("../../tests/fixtures/protocol_text_candidate.json").trim();
            let frame = serde_json::json!([[null, null, inner]]).to_string();
            let payload = format!("\n{frame}\n");
            let body = format!("{}{}", payload.encode_utf16().count(), payload);
            (StatusCode::OK, body).into_response()
        }

        let app = Router::new()
            .route("/", get(session_page))
            .route("/u/{auth_user}/app", get(session_page))
            .route(
                "/u/{auth_user}/_/BardChatUi/data/batchexecute",
                post(batch_ok),
            )
            .route(
                "/u/{auth_user}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate",
                post(generate),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let root = tempfile::tempdir().unwrap();
        let mut config = (*test_config(root.path())).clone();
        config.secure_1psid.clear();
        config.provider_origin = format!("http://{address}");
        let config = Arc::new(config);
        accounts::persist(
            &config.accounts_file,
            &[
                AccountCredentials {
                    id: "a".into(),
                    auth_user: 0,
                    secure_1psid: "cookie-a-value".into(),
                    secure_1psidts: None,
                    enabled: true,
                    weight: 1,
                },
                AccountCredentials {
                    id: "b".into(),
                    auth_user: 1,
                    secure_1psid: "cookie-b-value".into(),
                    secure_1psidts: None,
                    enabled: true,
                    weight: 1,
                },
            ],
        )
        .await
        .unwrap();
        let client = GeminiClient::initialize(config).await.unwrap();
        let output = client.generate("hello", "gemini-3-flash", 0).await.unwrap();
        assert_eq!(output.text, "fixture response text");
        let statuses = client.account_statuses().await;
        assert!(
            statuses
                .iter()
                .any(|status| status.id == "a" && status.cooldown_seconds > 0)
        );
        server.abort();
    }

    #[tokio::test]
    async fn rotated_cookie_is_absorbed_and_persisted() {
        // Google rotates 1PSIDTS through Set-Cookie. Without absorbing it the
        // gateway keeps replaying the captured value until it expires, and no
        // amount of retrying recovers, since only an external sync can help.
        async fn session_page() -> axum::response::Response {
            (
                StatusCode::OK,
                [("set-cookie", "__Secure-1PSIDTS=rotated-by-google; Path=/")],
                r#"{"SNlM0e":"token","cfb2h":"build","FdrFJe":"session","TuX5cc":"en"}"#,
            )
                .into_response()
        }
        async fn batch_ok() -> &'static str {
            "ok"
        }

        let app = Router::new()
            .route("/", get(session_page))
            .route("/u/{auth_user}/app", get(session_page))
            .route(
                "/u/{auth_user}/_/BardChatUi/data/batchexecute",
                post(batch_ok),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let root = tempfile::tempdir().unwrap();
        let mut config = (*test_config(root.path())).clone();
        config.secure_1psid.clear();
        config.provider_origin = format!("http://{address}");
        let config = Arc::new(config);
        accounts::persist(
            &config.accounts_file,
            &[AccountCredentials {
                id: "a".into(),
                auth_user: 0,
                secure_1psid: "cookie-a-value".into(),
                secure_1psidts: Some("stale-psidts".into()),
                enabled: true,
                weight: 1,
            }],
        )
        .await
        .unwrap();

        GeminiClient::initialize(config.clone()).await.unwrap();
        let stored = accounts::load(&config).unwrap();
        server.abort();

        assert_eq!(
            stored[0].secure_1psidts.as_deref(),
            Some("rotated-by-google"),
            "the rotated cookie should have replaced the stale one on disk"
        );
    }
}
