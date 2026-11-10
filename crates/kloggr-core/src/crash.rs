//! Crash reports (klogg: `crash_handler/`).
//!
//! klogg collects Crashpad minidumps and, at the next start, offers to send
//! them to Sentry. kloggr has nowhere to send them yet: a panic writes a
//! plain-text report to `<config dir>/crashes/new/`, and the next start
//! shows it so it can be copied into a bug report (`docs/decisions.md`).
//! Once shown, a report moves up to `crashes/`, which keeps the last
//! [`KEEP`].

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Reports kept in `crashes/` after they were shown.
pub const KEEP: usize = 10;

/// What a panic leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The panic message.
    pub message: String,
    /// `file:line:column` of the panic, if known.
    pub location: Option<String>,
    /// The panicking thread's name.
    pub thread: String,
    pub backtrace: String,
}

impl Report {
    /// The report's text: the panic, then the build and the system, then
    /// the backtrace (as klogg's issue template: version, OS, concurrency).
    pub fn text(&self, unix_time: u64) -> String {
        let concurrency = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut out = format!("kloggr crashed: {}\n", self.message);
        if let Some(location) = &self.location {
            out += &format!("at {location}\n");
        }
        out += &format!("thread: {}\n\n", self.thread);
        out += &format!(
            "> kloggr {} ({} build) [built for {}-{}]\n",
            env!("CARGO_PKG_VERSION"),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
        out += &format!("> running on {}, concurrency {concurrency}\n", os_name());
        out += &format!("> time {unix_time} (seconds since 1970, UTC)\n\n");
        out += "Backtrace:\n";
        out += &self.backtrace;
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// The system's name, e.g. "Ubuntu 26.04 LTS" on Linux.
fn os_name() -> String {
    #[cfg(target_os = "linux")]
    if let Some(name) = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| pretty_name(&s))
    {
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
        return format!("{name} (linux {})", kernel.trim());
    }
    std::env::consts::OS.to_owned()
}

/// `PRETTY_NAME` from an os-release file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pretty_name(os_release: &str) -> Option<String> {
    os_release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .filter(|v| !v.is_empty())
}

fn crashes(config_dir: &Path) -> PathBuf {
    config_dir.join("crashes")
}

fn new_dir(config_dir: &Path) -> PathBuf {
    crashes(config_dir).join("new")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Writes `report` to `crashes/new/`. Called from the panic hook, so it
/// only does plain file IO.
pub fn save(config_dir: &Path, report: &Report) -> std::io::Result<PathBuf> {
    let dir = new_dir(config_dir);
    std::fs::create_dir_all(&dir)?;
    let time = now();
    // Two panics in one second (two threads) get their own files.
    let mut path = dir.join(format!("crash-{time}.txt"));
    for n in 1.. {
        if !path.exists() {
            break;
        }
        path = dir.join(format!("crash-{time}-{n}.txt"));
    }
    std::fs::write(&path, report.text(time))?;
    Ok(path)
}

/// Reports not shown yet, oldest first.
pub fn pending(config_dir: &Path) -> Vec<PathBuf> {
    reports_in(&new_dir(config_dir))
}

fn reports_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    // By modification time: names sort crash-…-10 before crash-…-2.
    files.sort_by_key(|p| {
        (
            std::fs::metadata(p).and_then(|m| m.modified()).ok(),
            p.clone(),
        )
    });
    files
}

/// Moves the pending reports to `crashes/`, where only the last [`KEEP`]
/// stay. Returns where they went.
pub fn mark_shown(config_dir: &Path) -> Vec<PathBuf> {
    let dir = crashes(config_dir);
    let moved = pending(config_dir)
        .into_iter()
        .filter_map(|p| {
            let to = dir.join(p.file_name()?);
            std::fs::rename(&p, &to).ok().map(|_| to)
        })
        .collect();
    let old = reports_in(&dir);
    for p in &old[..old.len().saturating_sub(KEEP)] {
        let _ = std::fs::remove_file(p);
    }
    moved
}

/// A link to a new GitHub issue for `report` (klogg: `IssueReporter`),
/// titled by its first line. Browsers and GitHub cut long links, so the
/// body holds at most `max_body` bytes of the report; the rest is left
/// for the user to paste.
pub fn issue_url(report: &str, max_body: usize) -> String {
    let first = report.lines().next().unwrap_or("kloggr crashed");
    let title = format!("Crash: {}", first.trim_start_matches("kloggr crashed: "));
    let mut body = String::from("#### What did you do?\n\n\n#### Crash report\n\n```\n");
    if report.len() <= max_body {
        body += report;
    } else {
        let mut end = max_body;
        while !report.is_char_boundary(end) {
            end -= 1;
        }
        body += &report[..end];
        body += "\n… (cut: paste the rest of the report here)";
    }
    body += "\n```\n";
    format!(
        "{}/issues/new?title={}&body={}",
        crate::REPOSITORY,
        percent_encode(&title),
        percent_encode(&body)
    )
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

/// Installs a panic hook that saves a [`Report`] before the default hook
/// prints the panic.
pub fn install_hook(config_dir: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());
        let report = Report {
            message,
            location: info.location().map(|l| l.to_string()),
            thread: std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_owned(),
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        };
        match save(&config_dir, &report) {
            Ok(path) => eprintln!("kloggr: crash report saved to {}", path.display()),
            Err(e) => eprintln!("kloggr: couldn't save a crash report: {e}"),
        }
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("kloggr-crash-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn report(message: &str) -> Report {
        Report {
            message: message.into(),
            location: Some("src/x.rs:1:2".into()),
            thread: "main".into(),
            backtrace: "0: frame".into(),
        }
    }

    #[test]
    fn text() {
        let text = report("boom").text(42);
        assert!(text.starts_with("kloggr crashed: boom\nat src/x.rs:1:2\nthread: main\n\n"));
        assert!(text.contains(&format!("> kloggr {} (", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains("> time 42 "));
        assert!(text.ends_with("Backtrace:\n0: frame\n"));
    }

    #[test]
    fn issue_link() {
        let url = issue_url("kloggr crashed: boom é\nat x.rs:1:2\n", 1000);
        assert!(url.starts_with("https://github.com/onionhead0708/kloggr/issues/new?title=Crash%3A%20boom%20%C3%A9&body="));
        assert!(url.contains("at%20x.rs%3A1%3A2"));
        // Cut on a character boundary.
        let long = issue_url(&"é".repeat(100), 51);
        assert!(long.contains("cut%3A%20paste"));
        assert_eq!(percent_encode("a b/~"), "a%20b%2F~");
    }

    #[test]
    fn pretty_name_from_os_release() {
        let release = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 26.04 LTS\"\nID=ubuntu\n";
        assert_eq!(pretty_name(release).as_deref(), Some("Ubuntu 26.04 LTS"));
        assert_eq!(pretty_name("ID=x\n"), None);
    }

    #[test]
    fn save_then_show_once() {
        let tmp = Temp::new("flow");
        assert!(pending(&tmp.0).is_empty());
        let a = save(&tmp.0, &report("one")).unwrap();
        let b = save(&tmp.0, &report("two")).unwrap();
        assert_ne!(a, b);
        assert_eq!(pending(&tmp.0), [a.clone(), b.clone()]);
        assert!(std::fs::read_to_string(&b).unwrap().contains("two"));
        let moved = mark_shown(&tmp.0);
        assert_eq!(moved.len(), 2);
        assert!(moved.iter().all(|p| p.parent() == Some(&crashes(&tmp.0))));
        assert!(pending(&tmp.0).is_empty());
    }

    #[test]
    fn hook_saves_a_report() {
        let tmp = Temp::new("hook");
        let previous = std::panic::take_hook();
        install_hook(tmp.0.clone());
        let result = std::thread::Builder::new()
            .name("crash-test".into())
            .spawn(|| panic!("test panic {}", 7))
            .unwrap()
            .join();
        let _ = std::panic::take_hook();
        std::panic::set_hook(previous);
        assert!(result.is_err());
        let pending = pending(&tmp.0);
        assert_eq!(pending.len(), 1);
        let text = std::fs::read_to_string(&pending[0]).unwrap();
        assert!(
            text.starts_with(
                "kloggr crashed: test panic 7
at "
            ),
            "{text}"
        );
        assert!(text.contains("thread: crash-test\n"), "{text}");
        assert!(text.contains("Backtrace:\n"), "{text}");
    }

    #[test]
    fn keeps_the_last_reports() {
        let tmp = Temp::new("keep");
        for i in 0..KEEP + 3 {
            save(&tmp.0, &report(&i.to_string())).unwrap();
        }
        mark_shown(&tmp.0);
        let kept = reports_in(&crashes(&tmp.0));
        assert_eq!(kept.len(), KEEP);
        // The oldest three went.
        let first = std::fs::read_to_string(&kept[0]).unwrap();
        assert!(first.starts_with("kloggr crashed: 3\n"), "{first}");
    }
}
