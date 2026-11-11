//! Downloading a log file from a URL (klogg: `Downloader`, used by
//! File > Open URL…).
//!
//! The file is streamed to `<dir>/<name>.part` and renamed once complete,
//! so a cancelled or failed download leaves nothing behind. Its name is the
//! URL's last path segment.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("{0} is not an http or https URL")]
    BadUrl(String),
    #[error("{0}")]
    Http(#[from] ureq::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("download cancelled")]
    Cancelled,
}

/// What was typed, as a URL: `https://` is added when no scheme is given
/// (klogg: `QUrl::fromUserInput`). `None` if it isn't http or https.
pub fn parse_url(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let url = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    // A host is needed.
    (!rest.is_empty() && !rest.starts_with('/')).then_some(url)
}

/// The file name for `url`: its last path segment, decoded, without
/// characters file systems refuse; "download" if there is none.
pub fn file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let after_host = path.split_once("://").map_or(path, |(_, rest)| rest);
    let segment = after_host
        .split_once('/')
        .map_or("", |(_, p)| p)
        .rsplit('/')
        .next()
        .unwrap_or("");
    let decoded = percent_encoding::percent_decode_str(segment).decode_utf8_lossy();
    let name: String = decoded
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let name = name.trim_matches(['.', ' ']);
    if name.is_empty() {
        "download".to_owned()
    } else {
        name.to_owned()
    }
}

/// Downloads `url` into `dir`, reporting the bytes received and the total
/// size if the server tells it, and stopping once `cancel` is set. Returns
/// the file's path: `dir/<name>`, or `dir/<stem>-2.<ext>`… if taken.
pub fn download(
    url: &str,
    dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, DownloadError> {
    let url = parse_url(url).ok_or_else(|| DownloadError::BadUrl(url.to_owned()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        // Large files take long: only a stalled transfer is an error.
        .timeout_per_read(Some(Duration::from_secs(60)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let response = agent
        .get(&url)
        .header("User-Agent", concat!("kloggr/", env!("CARGO_PKG_VERSION")))
        .call()?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    std::fs::create_dir_all(dir)?;
    let path = free_path(dir, &file_name(&url));
    let part = path.with_file_name(format!(
        "{}.part",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let result = (|| {
        let mut reader = response.into_body().into_reader();
        let mut out = std::io::BufWriter::new(std::fs::File::create(&part)?);
        let mut buf = vec![0; 256 * 1024];
        let mut received = 0u64;
        progress(0, total);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(DownloadError::Cancelled);
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            received += n as u64;
            progress(received, total);
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&part, &path)?;
        Ok(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// `dir/name`, or `dir/stem-2.ext`, `-3`… if taken.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}{ext}")))
        .find(|p| !p.exists())
        .expect("a free name")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    #[test]
    fn urls() {
        assert_eq!(
            parse_url(" example.com/a.log ").as_deref(),
            Some("https://example.com/a.log")
        );
        assert_eq!(
            parse_url("http://h/x").as_deref(),
            Some("http://h/x"),
            "kept as given"
        );
        assert_eq!(parse_url("ftp://h/x"), None);
        assert_eq!(parse_url("https://"), None);
        assert_eq!(parse_url("two words"), None);
        assert_eq!(parse_url(""), None);
    }

    #[test]
    fn names() {
        assert_eq!(file_name("https://h/logs/app.log?x=1#y"), "app.log");
        assert_eq!(file_name("https://h/logs/my%20app.log"), "my app.log");
        assert_eq!(file_name("https://h/a%2Fb%3A.log"), "a_b_.log");
        assert_eq!(file_name("https://h/"), "download");
        assert_eq!(file_name("https://h"), "download");
        assert_eq!(file_name("https://h/.."), "download");
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("kloggr-download-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Serves one request with `status` and `body`, then closes.
    fn serve_once(status: &'static str, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn downloads_to_a_free_name() {
        let dir = Dir::new("ok");
        let body: Vec<u8> = (0..300_000).map(|i| b'a' + (i % 26) as u8).collect();
        let url = serve_once("200 OK", body.clone());
        let mut seen = Vec::new();
        let path = download(
            &format!("{url}/logs/app.log"),
            &dir.0,
            &AtomicBool::new(false),
            |got, total| seen.push((got, total)),
        )
        .unwrap();
        assert_eq!(path, dir.0.join("app.log"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(seen.first(), Some(&(0, Some(300_000))));
        assert_eq!(seen.last(), Some(&(300_000, Some(300_000))));

        let url = serve_once("200 OK", b"x\n".to_vec());
        let again = download(
            &format!("{url}/logs/app.log"),
            &dir.0,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(again, dir.0.join("app-2.log"));
    }

    #[test]
    fn errors_and_cancel_leave_nothing() {
        let dir = Dir::new("err");
        let url = serve_once("404 Not Found", b"no".to_vec());
        let err = download(
            &format!("{url}/a.log"),
            &dir.0,
            &AtomicBool::new(false),
            |_, _| {},
        );
        assert!(matches!(
            err,
            Err(DownloadError::Http(ureq::Error::StatusCode(404)))
        ));

        let url = serve_once("200 OK", vec![b'x'; 10_000]);
        let cancel = AtomicBool::new(true);
        let err = download(&format!("{url}/a.log"), &dir.0, &cancel, |_, _| {});
        assert!(matches!(err, Err(DownloadError::Cancelled)));
        let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");

        assert!(matches!(
            download("ftp://h/a", &dir.0, &AtomicBool::new(false), |_, _| {}),
            Err(DownloadError::BadUrl(_))
        ));
    }
}
