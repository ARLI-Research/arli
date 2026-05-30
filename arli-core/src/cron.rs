//! Cron scheduler — run tasks on repeating schedules.
//!
//! Similar to Hermes' cronjob system:
//! - Jobs have a schedule (cron expression or interval)
//! - Jobs have a prompt (what agent runs each tick)
//! - Jobs can be paused, resumed, removed
//! - Jobs deliver results to a target

use chrono::{DateTime, Utc};
use cron::Schedule;
use std::collections::HashMap;
use std::str::FromStr;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::error::{Error, Result};

/// A cron job definition.
#[derive(Debug, Clone)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule_str: String,
    pub prompt: String,
    pub deliver: Option<String>,
    pub skills: Vec<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub run_count: u64,
    pub error_count: u64,
}

/// Status update from a running job.
#[derive(Debug, Clone)]
pub enum CronEvent {
    /// A job fired and is running.
    JobRunning { job_id: String },
    /// A job completed successfully.
    JobCompleted { job_id: String, output: String },
    /// A job failed.
    JobFailed { job_id: String, error: String },
    /// The scheduler's tick.
    Tick,
}

/// The cron scheduler — manages scheduled tasks.
pub struct CronScheduler {
    jobs: Mutex<HashMap<String, CronJobHandle>>,
    tx: broadcast::Sender<CronEvent>,
}

struct CronJobHandle {
    job: CronJob,
    cancel_tx: tokio::sync::watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl CronScheduler {
    /// Create a new empty scheduler.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            jobs: Mutex::new(HashMap::new()),
            tx,
        }
    }

    /// Subscribe to cron events (JobRunning, JobCompleted, JobFailed).
    pub fn subscribe(&self) -> broadcast::Receiver<CronEvent> {
        self.tx.subscribe()
    }

    /// Add a new job with a cron expression (e.g. "0 */5 * * * *" or "every 30s").
    pub async fn add_job(&self, job: CronJob) -> Result<()> {
        let schedule = parse_schedule(&job.schedule_str)?;

        let job_id = job.id.clone();
        let prompt = job.prompt.clone();
        let job_id_for_insert = job.id.clone();
        let event_tx = self.tx.clone();

        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move {
            run_job_loop(job_id, prompt, schedule, cancel_rx, event_tx).await;
        });

        let handle_wrapper = CronJobHandle {
            job,
            cancel_tx,
            task: handle,
        };

        self.jobs.lock().await.insert(job_id_for_insert, handle_wrapper);
        Ok(())
    }

    /// Remove a job by ID.
    pub async fn remove_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.lock().await;
        if let Some(handle) = jobs.remove(job_id) {
            handle.cancel_tx.send(true).ok();
            handle.task.abort();
            tracing::info!("Cron job removed: {}", job_id);
            Ok(())
        } else {
            Err(Error::Tool(format!("Job not found: {}", job_id)))
        }
    }

    /// Pause a job.
    pub async fn pause_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.lock().await;
        if let Some(handle) = jobs.get_mut(job_id) {
            handle.job.enabled = false;
            handle.cancel_tx.send(false).ok(); // Does NOT abort — just suspends
            tracing::info!("Cron job paused: {}", job_id);
            Ok(())
        } else {
            Err(Error::Tool(format!("Job not found: {}", job_id)))
        }
    }

    /// Resume a paused job.
    pub async fn resume_job(&self, job_id: &str) -> Result<()> {
        let mut jobs = self.jobs.lock().await;
        if let Some(handle) = jobs.get_mut(job_id) {
            handle.job.enabled = true;
            tracing::info!("Cron job resumed: {}", job_id);
            Ok(())
        } else {
            Err(Error::Tool(format!("Job not found: {}", job_id)))
        }
    }

    /// List all jobs.
    pub async fn list_jobs(&self) -> Vec<CronJob> {
        let jobs = self.jobs.lock().await;
        jobs.values().map(|h| h.job.clone()).collect()
    }

    /// Run a job immediately (for testing).
    pub async fn run_now(&self, job_id: &str) -> Result<()> {
        let jobs = self.jobs.lock().await;
        if let Some(handle) = jobs.get(job_id) {
            let job_id = job_id.to_string();
            let prompt = handle.job.prompt.clone();
            let event_tx = self.tx.clone();

            tokio::spawn(async move {
                let _ = event_tx.send(CronEvent::JobRunning { job_id: job_id.clone() });

                match execute_job(&job_id, &prompt).await {
                    Ok(output) => {
                        let _ = event_tx.send(CronEvent::JobCompleted { job_id, output });
                    }
                    Err(e) => {
                        let _ = event_tx.send(CronEvent::JobFailed { job_id, error: e.to_string() });
                    }
                }
            });
            Ok(())
        } else {
            Err(Error::Tool(format!("Job not found: {}", job_id)))
        }
    }
}

/// Background loop for a single cron job.
async fn run_job_loop(
    job_id: String,
    prompt: String,
    schedule: Schedule,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    event_tx: broadcast::Sender<CronEvent>,
) {
    use tokio::time::{sleep, Duration};

    loop {
        // Check if cancelled
        if *cancel_rx.borrow() {
            break;
        }

        // Wait until next schedule time
        if let Some(next) = schedule.upcoming(Utc).next() {
            let now = Utc::now();
            if next > now {
                let delay = (next - now).to_std().unwrap_or(Duration::from_secs(60));
                tokio::select! {
                    _ = sleep(delay) => {},
                    _ = cancel_rx.changed() => {
                        if *cancel_rx.borrow() {
                            break;
                        }
                        continue;
                    }
                }
            }

            let _ = event_tx.send(CronEvent::JobRunning {
                job_id: job_id.clone(),
            });

            match execute_job(&job_id, &prompt).await {
                Ok(output) => {
                    let _ = event_tx.send(CronEvent::JobCompleted {
                        job_id: job_id.clone(),
                        output,
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(CronEvent::JobFailed {
                        job_id: job_id.clone(),
                        error: e.to_string(),
                    });
                }
            }
        } else {
            // No upcoming times — schedule is exhausted
            break;
        }
    }
}

/// Execute a single job (placeholder — real impl spawns an agent).
async fn execute_job(job_id: &str, prompt: &str) -> Result<String> {
    // In the real implementation, this would spawn a child Agent with the prompt.
    // For now, return a placeholder.
    tracing::info!("Cron job '{}' executing: {}", job_id, prompt);
    Ok(format!("Job '{}' processed prompt: '{}'", job_id, prompt))
}

/// Parse a cron expression or human-readable interval.
fn parse_schedule(spec: &str) -> Result<Schedule> {
    // Try standard cron expression first
    if spec.contains('*') || spec.split_whitespace().count() >= 5 {
        return Schedule::from_str(spec)
            .map_err(|e| Error::Tool(format!("Invalid cron expression '{}': {}", spec, e)));
    }

    // Human-readable intervals: "30s", "5m", "1h", "every 2h"
    let cleaned = spec
        .trim()
        .trim_start_matches("every ")
        .trim();

    if let Some(secs) = parse_interval(cleaned) {
        // Convert to cron: "every N seconds"
        if secs < 60 {
            let expr = format!("*/{} * * * * *", secs.max(1));
            Schedule::from_str(&expr)
                .map_err(|e| Error::Tool(format!("Invalid interval '{}': {}", spec, e)))
        } else if secs < 3600 {
            let mins = secs / 60;
            let expr = format!("0 */{} * * * *", mins);
            Schedule::from_str(&expr)
                .map_err(|e| Error::Tool(format!("Invalid interval '{}': {}", spec, e)))
        } else {
            let hours = secs / 3600;
            let expr = format!("0 0 */{} * * *", hours);
            Schedule::from_str(&expr)
                .map_err(|e| Error::Tool(format!("Invalid interval '{}': {}", spec, e)))
        }
    } else {
        Err(Error::Tool(format!(
            "Cannot parse schedule '{}'. Use cron expression or interval like '30s', '5m', '1h'",
            spec
        )))
    }
}

fn parse_interval(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.ends_with('s') {
        s[..s.len() - 1].parse().ok()
    } else if s.ends_with('m') {
        s[..s.len() - 1].parse::<u64>().ok().map(|v| v * 60)
    } else if s.ends_with('h') {
        s[..s.len() - 1].parse::<u64>().ok().map(|v| v * 3600)
    } else {
        s.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_schedule_cron() {
        assert!(parse_schedule("0 */5 * * * *").is_ok());
        assert!(parse_schedule("0 0 9 * * *").is_ok());
    }

    #[test]
    fn test_parse_schedule_intervals() {
        let s = parse_schedule("30s").unwrap();
        assert!(s.upcoming(Utc).next().is_some());

        let s = parse_schedule("5m").unwrap();
        assert!(s.upcoming(Utc).next().is_some());

        let s = parse_schedule("every 2h").unwrap();
        assert!(s.upcoming(Utc).next().is_some());
    }

    #[test]
    fn test_parse_schedule_invalid() {
        assert!(parse_schedule("not a schedule").is_err());
    }

    #[tokio::test]
    async fn test_add_and_remove_job() {
        let scheduler = CronScheduler::new();

        let job = CronJob {
            id: "test-1".to_string(),
            name: "Test Job".to_string(),
            schedule_str: "0 */5 * * * *".to_string(),
            prompt: "echo hello".to_string(),
            deliver: None,
            skills: vec![],
            enabled: true,
            created_at: Utc::now(),
            last_run_at: None,
            next_run_at: None,
            run_count: 0,
            error_count: 0,
        };

        scheduler.add_job(job).await.unwrap();
        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 1);

        scheduler.remove_job("test-1").await.unwrap();
        let jobs = scheduler.list_jobs().await;
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn test_pause_and_resume() {
        let scheduler = CronScheduler::new();

        let job = CronJob {
            id: "test-2".to_string(),
            name: "Pausable".to_string(),
            schedule_str: "0 */5 * * * *".to_string(),
            prompt: "test".to_string(),
            deliver: None,
            skills: vec![],
            enabled: true,
            created_at: Utc::now(),
            last_run_at: None,
            next_run_at: None,
            run_count: 0,
            error_count: 0,
        };

        scheduler.add_job(job).await.unwrap();
        scheduler.pause_job("test-2").await.unwrap();

        let jobs = scheduler.list_jobs().await;
        assert!(!jobs[0].enabled);

        scheduler.resume_job("test-2").await.unwrap();
        let jobs = scheduler.list_jobs().await;
        assert!(jobs[0].enabled);

        scheduler.remove_job("test-2").await.unwrap();
    }
}
