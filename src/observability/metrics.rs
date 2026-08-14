use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub requests_total: Vec<Counter>,
    pub request_duration_seconds: Vec<Histogram>,
    pub tasks_queued: u64,
    pub tasks_running: u64,
    pub tasks_completed_total: u64,
    pub tasks_failed_total: u64,
    pub provider_errors_total: u64,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Counter {
    pub labels: HashMap<String, String>,
    pub value: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Histogram {
    pub labels: HashMap<String, String>,
    pub count: u64,
    pub sum_seconds: f64,
    pub buckets: Vec<Bucket>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    pub le: f64,
    pub count: u64,
}

const BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

#[derive(Default)]
struct RequestStat {
    count: u64,
    sum: f64,
    buckets: [u64; BUCKETS.len()],
}

pub struct Metrics {
    started_at: Instant,
    requests: RwLock<HashMap<String, RequestStat>>,
    tasks_queued: AtomicU64,
    tasks_running: AtomicU64,
    tasks_completed: AtomicU64,
    tasks_failed: AtomicU64,
    provider_errors: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started_at: Instant::now(),
            requests: RwLock::new(HashMap::new()),
            tasks_queued: AtomicU64::new(0),
            tasks_running: AtomicU64::new(0),
            tasks_completed: AtomicU64::new(0),
            tasks_failed: AtomicU64::new(0),
            provider_errors: AtomicU64::new(0),
        })
    }

    pub fn record_request(&self, method: &str, path: &str, status: u16, duration: Duration) {
        let key = format!("{method}|{path}|{status}");
        let mut requests = self.requests.write().expect("metrics lock poisoned");
        let stat = requests.entry(key).or_default();
        stat.count += 1;
        stat.sum += duration.as_secs_f64();
        let seconds = duration.as_secs_f64();
        for (index, le) in BUCKETS.iter().enumerate() {
            if seconds <= *le {
                stat.buckets[index] += 1;
            }
        }
    }

    pub fn inc_queued(&self) {
        self.tasks_queued.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_queued(&self) {
        self.tasks_queued
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
    }

    pub fn inc_running(&self) {
        self.tasks_running.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_running(&self) {
        self.tasks_running
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
    }

    pub fn inc_completed(&self) {
        self.tasks_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_failed(&self) {
        self.tasks_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_provider_error(&self) {
        self.provider_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Snapshot {
        let requests = self.requests.read().expect("metrics lock poisoned");
        let mut requests_total = Vec::new();
        let mut request_duration_seconds = Vec::new();
        for (key, stat) in requests.iter() {
            let mut parts = key.splitn(3, '|');
            let method = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("");
            let status = parts.next().unwrap_or("");
            let mut labels = HashMap::new();
            labels.insert("method".into(), method.into());
            labels.insert("path".into(), path.into());
            labels.insert("status".into(), status.into());
            requests_total.push(Counter {
                labels: labels.clone(),
                value: stat.count,
            });
            let buckets = BUCKETS
                .iter()
                .zip(stat.buckets.iter())
                .map(|(le, count)| Bucket {
                    le: *le,
                    count: *count,
                })
                .collect();
            request_duration_seconds.push(Histogram {
                labels,
                count: stat.count,
                sum_seconds: stat.sum,
                buckets,
            });
        }
        Snapshot {
            requests_total,
            request_duration_seconds,
            tasks_queued: self.tasks_queued.load(Ordering::Relaxed),
            tasks_running: self.tasks_running.load(Ordering::Relaxed),
            tasks_completed_total: self.tasks_completed.load(Ordering::Relaxed),
            tasks_failed_total: self.tasks_failed.load(Ordering::Relaxed),
            provider_errors_total: self.provider_errors.load(Ordering::Relaxed),
            uptime_seconds: self.started_at.elapsed().as_secs(),
        }
    }

    pub fn render_prometheus(&self) -> String {
        let snapshot = self.snapshot();
        let mut output = String::new();
        output.push_str("# HELP gateway_requests_total Total HTTP requests handled.\n");
        output.push_str("# TYPE gateway_requests_total counter\n");
        for counter in &snapshot.requests_total {
            output.push_str(&format!(
                "gateway_requests_total{{{}}} {}\n",
                format_labels(&counter.labels),
                counter.value
            ));
        }
        output.push_str("# HELP gateway_request_duration_seconds HTTP request latency.\n");
        output.push_str("# TYPE gateway_request_duration_seconds histogram\n");
        for histogram in &snapshot.request_duration_seconds {
            let base = format_labels(&histogram.labels);
            for bucket in &histogram.buckets {
                let mut labels = histogram.labels.clone();
                labels.insert("le".into(), bucket.le.to_string());
                output.push_str(&format!(
                    "gateway_request_duration_seconds_bucket{{{}}} {}\n",
                    format_labels(&labels),
                    bucket.count
                ));
            }
            output.push_str(&format!(
                "gateway_request_duration_seconds_count{{{}}} {}\n",
                base, histogram.count
            ));
            output.push_str(&format!(
                "gateway_request_duration_seconds_sum{{{}}} {}\n",
                base, histogram.sum_seconds
            ));
        }
        output.push_str("# HELP gateway_tasks_queued Current queued generation tasks.\n");
        output.push_str("# TYPE gateway_tasks_queued gauge\n");
        output.push_str(&format!("gateway_tasks_queued {}\n", snapshot.tasks_queued));
        output.push_str("# HELP gateway_tasks_running Current running generation tasks.\n");
        output.push_str("# TYPE gateway_tasks_running gauge\n");
        output.push_str(&format!(
            "gateway_tasks_running {}\n",
            snapshot.tasks_running
        ));
        output.push_str("# HELP gateway_tasks_completed_total Total completed generation tasks.\n");
        output.push_str("# TYPE gateway_tasks_completed_total counter\n");
        output.push_str(&format!(
            "gateway_tasks_completed_total {}\n",
            snapshot.tasks_completed_total
        ));
        output.push_str("# HELP gateway_tasks_failed_total Total failed generation tasks.\n");
        output.push_str("# TYPE gateway_tasks_failed_total counter\n");
        output.push_str(&format!(
            "gateway_tasks_failed_total {}\n",
            snapshot.tasks_failed_total
        ));
        output.push_str("# HELP gateway_provider_errors_total Total upstream provider errors.\n");
        output.push_str("# TYPE gateway_provider_errors_total counter\n");
        output.push_str(&format!(
            "gateway_provider_errors_total {}\n",
            snapshot.provider_errors_total
        ));
        output.push_str("# HELP gateway_uptime_seconds Seconds since the gateway started.\n");
        output.push_str("# TYPE gateway_uptime_seconds gauge\n");
        output.push_str(&format!(
            "gateway_uptime_seconds {}\n",
            snapshot.uptime_seconds
        ));
        output
    }
}

fn format_labels(labels: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", escape_label(v)))
        .collect();
    parts.sort();
    parts.join(",")
}

fn escape_label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_renders_metrics() {
        let metrics = Metrics::new();
        metrics.record_request("GET", "/health", 200, Duration::from_millis(10));
        metrics.record_request(
            "POST",
            "/v1/chat/completions",
            200,
            Duration::from_millis(250),
        );
        metrics.inc_queued();
        metrics.inc_running();
        metrics.inc_completed();
        let output = metrics.render_prometheus();
        assert!(output.contains("gateway_requests_total"));
        assert!(output.contains("gateway_tasks_queued 1"));
        assert!(output.contains("gateway_tasks_running 1"));
        assert!(output.contains("gateway_tasks_completed_total 1"));
    }
}
