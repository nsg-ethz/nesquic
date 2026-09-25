//! Describes the monitored process: whether it is an IUT at all, and the
//! job, labels and library it runs, which become InfluxDB tags.
//!
//! Everything here is derived from the process itself (its executable and
//! command line, see docs/CLI.md) so the IUTs need no extra flags.

use std::collections::BTreeMap;
use std::path::Path;

/// IUT binaries are named `nesquic-<library>` (see the Dockerfiles).
const BIN_PREFIX: &str = "nesquic-";

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub job: Option<String>,
    pub mode: Option<String>,
    pub library: Option<String>,
    pub labels: BTreeMap<String, String>,
}

/// Whether this process should be monitored: `NQ_ENABLE=1`/`0` forces the
/// decision, otherwise only `nesquic-*` binaries are. The library is loaded
/// into every process via `/etc/ld.so.preload`, including the shell and
/// mahimahi's `mm-*` wrappers, which must not report anything.
pub(crate) fn enabled() -> bool {
    match std::env::var("NQ_ENABLE").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => std::fs::read_link("/proc/self/exe").is_ok_and(|exe| is_iut(&exe)),
    }
}

fn exe_name() -> Option<String> {
    let exe = std::fs::read_link("/proc/self/exe").ok()?;
    Some(exe.file_name()?.to_string_lossy().into_owned())
}

fn cmdline() -> Vec<String> {
    let Ok(raw) = std::fs::read("/proc/self/cmdline") else {
        return vec![];
    };
    raw.split(|&b| b == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect()
}

impl Context {
    pub fn current() -> Self {
        let mut ctx = Self::from_args(&cmdline());
        ctx.library = std::env::var("NQ_LIBRARY").ok().or_else(|| {
            exe_name().and_then(|name| name.strip_prefix(BIN_PREFIX).map(str::to_string))
        });
        ctx
    }

    /// Parses the container CLI: `<bin> <client|server> [-j JOB] [-L k:v]...`.
    fn from_args(args: &[String]) -> Self {
        let mut ctx = Context {
            mode: args
                .get(1)
                .filter(|m| *m == "client" || *m == "server")
                .cloned(),
            ..Default::default()
        };

        let mut args = args.iter().skip(1);
        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
                _ => (arg.as_str(), None),
            };

            let value = match (flag, inline) {
                ("-j" | "--job" | "-L" | "--labels", Some(v)) => Some(v.to_string()),
                ("-j" | "--job" | "-L" | "--labels", None) => args.next().cloned(),
                (f, _) if f.len() > 2 && (f.starts_with("-j") || f.starts_with("-L")) => {
                    Some(f[2..].trim_start_matches('=').to_string())
                }
                _ => continue,
            };
            let Some(value) = value else { break };

            if flag.starts_with("-j") || flag == "--job" {
                ctx.job = Some(value);
            } else if let Some((k, v)) = value.split_once(':') {
                ctx.labels
                    .insert(k.trim().to_string(), v.trim().to_string());
            }
        }

        ctx
    }

    /// All InfluxDB tags for this process, sorted by key.
    pub fn tags(&self) -> BTreeMap<String, String> {
        let mut tags = BTreeMap::new();
        let version = std::env::var("NQ_LIBRARY_VERSION").ok();
        for (key, value) in [
            ("job", &self.job),
            ("mode", &self.mode),
            ("library", &self.library),
            ("version", &version),
        ] {
            if let Some(value) = value {
                tags.insert(key.to_string(), value.clone());
            }
        }
        tags.extend(self.labels.clone());
        tags
    }
}

fn is_iut(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with(BIN_PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn parses_client_cli() {
        let ctx = Context::from_args(&args(
            "nesquic-quinn client -j delay5 --cert c.pem --blob 50Mbit https://10.0.0.1:4433 -L nesquic_run:first -L host: a",
        ));
        assert_eq!(ctx.mode.as_deref(), Some("client"));
        assert_eq!(ctx.job.as_deref(), Some("delay5"));
        assert_eq!(
            ctx.labels.get("nesquic_run").map(String::as_str),
            Some("first")
        );
        assert_eq!(ctx.labels.get("host").map(String::as_str), Some(""));
    }

    #[test]
    fn parses_inline_values() {
        let ctx = Context::from_args(&args("nesquic-quiche server --job=x -Lrun:y -jz"));
        assert_eq!(ctx.mode.as_deref(), Some("server"));
        assert_eq!(ctx.job.as_deref(), Some("z"));
        assert_eq!(ctx.labels.get("run").map(String::as_str), Some("y"));
    }

    #[test]
    fn no_job() {
        let ctx = Context::from_args(&args("nesquic-noq server --cert c --key k"));
        assert_eq!(ctx.job, None);
        assert!(ctx.labels.is_empty());
    }

    #[test]
    fn detects_iut_binaries() {
        assert!(is_iut(Path::new("/usr/local/bin/nesquic-quinn")));
        assert!(!is_iut(Path::new("/usr/local/bin/mm-delay")));
        assert!(!is_iut(Path::new("/bin/bash")));
    }
}
