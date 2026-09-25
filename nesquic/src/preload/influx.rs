//! Uploads the collected metrics to InfluxDB v2.

use std::time::Duration;

/// Where to write, from the `INFLUX_*` environment variables that
/// script/run.sh passes to every IUT container.
pub(crate) struct Influx {
    url: String,
    token: String,
    org: String,
    bucket: String,
}

impl Influx {
    /// `None` unless all of `INFLUX_URL`, `INFLUX_TOKEN`, `INFLUX_ORG` and
    /// `INFLUX_BUCKET` are set.
    pub fn from_env() -> Option<Self> {
        let var = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
        Some(Influx {
            url: var("INFLUX_URL")?,
            token: var("INFLUX_TOKEN")?,
            org: var("INFLUX_ORG")?,
            bucket: var("INFLUX_BUCKET")?,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Writes `body` (line protocol, nanosecond timestamps).
    ///
    /// Runs at process exit, so it is blocking and bounded: `docker stop`
    /// sends SIGKILL two seconds after SIGTERM (see script/run.sh).
    pub fn write(&self, body: String) -> Result<(), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(1500)))
            .http_status_as_error(false)
            .build()
            .into();

        let write_url = format!(
            "{}/api/v2/write?org={}&bucket={}&precision=ns",
            self.url.trim_end_matches('/'),
            self.org,
            self.bucket
        );

        let mut resp = agent
            .post(&write_url)
            .header("Authorization", &format!("Token {}", self.token))
            .header("Content-Type", "text/plain; charset=utf-8")
            .send(body)
            .map_err(|e| format!("request to {write_url} failed: {e}"))?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let msg = resp.body_mut().read_to_string().unwrap_or_default();
            Err(format!("InfluxDB responded with {status}: {msg}"))
        }
    }
}
