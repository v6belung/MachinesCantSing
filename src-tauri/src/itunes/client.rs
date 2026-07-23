use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tokio::time::sleep;

/// iTunes allows ~20 req/min per IP; ~4s spacing keeps us safely under that (docs/phase0-plan.md §3.4).
const MIN_SPACING: Duration = Duration::from_secs(4);
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);
const MAX_TRANSIENT_RETRIES: u32 = 5;
const USER_AGENT: &str = "NowPlayingFlagger/0.1 (Phase1; +mailto:288729508+v6belung@users.noreply.github.com)";

/// Global rate limiter for the iTunes Search API: a single-worker serial queue.
/// The gate mutex is held for an item's *entire* processing — including any
/// backoff sleeps — so a 403 pauses the whole queue, not just the current item,
/// matching "back off ~60s, then resume the queue" in the plan.
pub struct ItunesClient {
    http: reqwest::Client,
    gate: Mutex<Instant>,
}

impl ItunesClient {
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self {
            http,
            // Allow the very first request to fire immediately.
            gate: Mutex::new(Instant::now() - MIN_SPACING),
        })
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> anyhow::Result<T> {
        let resp = self.throttled_get(url).await?;
        Ok(resp.json::<T>().await?)
    }

    async fn throttled_get(&self, url: &str) -> anyhow::Result<reqwest::Response> {
        let mut last_started = self.gate.lock().await;

        let elapsed = last_started.elapsed();
        if elapsed < MIN_SPACING {
            sleep(MIN_SPACING - elapsed).await;
        }

        let mut attempt: u32 = 0;
        let result = loop {
            let sent = self.http.get(url).send().await;
            match sent {
                Ok(resp) if resp.status() == StatusCode::FORBIDDEN => {
                    log::warn!(
                        "iTunes API rate-limited (403); backing off {RATE_LIMIT_BACKOFF:?} before resuming queue"
                    );
                    sleep(RATE_LIMIT_BACKOFF).await;
                    continue;
                }
                Ok(resp) if resp.status().is_server_error() => {
                    attempt += 1;
                    if attempt > MAX_TRANSIENT_RETRIES {
                        break Err(anyhow::anyhow!(
                            "iTunes API persistent server error: {}",
                            resp.status()
                        ));
                    }
                    let backoff = transient_backoff(attempt);
                    log::warn!(
                        "iTunes API server error {}; retrying in {backoff:?}",
                        resp.status()
                    );
                    sleep(backoff).await;
                    continue;
                }
                Ok(resp) => match resp.error_for_status() {
                    Ok(resp) => break Ok(resp),
                    Err(err) => break Err(err.into()),
                },
                Err(err) => {
                    attempt += 1;
                    if attempt > MAX_TRANSIENT_RETRIES {
                        break Err(err.into());
                    }
                    let backoff = transient_backoff(attempt);
                    log::warn!("iTunes API network error ({err}); retrying in {backoff:?}");
                    sleep(backoff).await;
                    continue;
                }
            }
        };

        *last_started = Instant::now();
        result
    }
}

/// Exponential backoff starting at 5s, capped at 60s (docs/phase0-plan.md §3.4).
fn transient_backoff(attempt: u32) -> Duration {
    let exp = attempt.min(4) - 1; // caps growth before the Duration cap kicks in
    Duration::from_secs(5 * (1u64 << exp)).min(RATE_LIMIT_BACKOFF)
}
