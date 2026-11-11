//! One kloggr at a time (klogg: `SingleApplication`, and
//! `sendFilesToPrimaryInstance`).
//!
//! The first kloggr listens on a loopback TCP port and writes the port and
//! a random token to `instance` in the config directory, readable only by
//! its owner. A later kloggr reads it, connects, sends the token and the
//! files it was given, and exits once they arrive. The token keeps other
//! local users' programs from making kloggr open files. A stale file (the
//! first kloggr crashed) just fails to connect: the new kloggr takes over.
//!
//! The message is text: the token, then one absolute path per line, then an
//! empty line. The answer is `ok`.

use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FILE: &str = "instance";
const TIMEOUT: Duration = Duration::from_secs(2);

/// Sends `files` (made absolute) to the running kloggr. `Ok(true)` if one
/// received them; `Ok(false)` if none is running.
pub fn send_to_running(config_dir: &Path, files: &[PathBuf]) -> std::io::Result<bool> {
    let Some((port, token)) = read_instance_file(config_dir) else {
        return Ok(false);
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, TIMEOUT) else {
        return Ok(false);
    };
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut message = format!("{token}\n");
    for file in files {
        let file = std::path::absolute(file).unwrap_or_else(|_| file.clone());
        // A path with a line break can't be sent; such names don't occur
        // in practice.
        let file = file.to_string_lossy();
        if !file.contains(['\n', '\r']) {
            message += &file;
            message.push('\n');
        }
    }
    message.push('\n');
    stream.write_all(message.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer)?;
    Ok(answer.trim() == "ok")
}

/// The first kloggr's end: receives the files later launches send.
pub struct Server {
    listener: TcpListener,
    token: String,
    file: PathBuf,
}

impl Server {
    /// Listens, and writes the instance file so that later launches find
    /// this one.
    pub fn start(config_dir: &Path) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let token = random_token();
        std::fs::create_dir_all(config_dir)?;
        let file = config_dir.join(FILE);
        write_private(&file, &format!("{port} {token}\n"))?;
        Ok(Self {
            listener,
            token,
            file,
        })
    }

    /// Waits for launches (blocking: run it on a thread), and hands the
    /// files of each to `received`. Returns when listening fails.
    pub fn serve(&self, mut received: impl FnMut(Vec<PathBuf>)) {
        for stream in self.listener.incoming() {
            let Ok(stream) = stream else { continue };
            match self.read_message(stream) {
                Ok(Some(files)) => received(files),
                Ok(None) => tracing::warn!("ignored a message with a wrong token"),
                Err(e) => tracing::warn!("cannot read a message from another kloggr: {e}"),
            }
        }
    }

    fn read_message(&self, stream: TcpStream) -> std::io::Result<Option<Vec<PathBuf>>> {
        stream.set_read_timeout(Some(TIMEOUT))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut token = String::new();
        reader.read_line(&mut token)?;
        if token.trim_end() != self.token {
            return Ok(None);
        }
        let mut files = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                break;
            }
            files.push(PathBuf::from(line));
        }
        (&stream).write_all(b"ok\n")?;
        Ok(Some(files))
    }

    /// Removes the instance file, if it is still this kloggr's.
    pub fn stop(&self) {
        let ours = std::fs::read_to_string(&self.file)
            .is_ok_and(|s| s.split_whitespace().nth(1) == Some(self.token.as_str()));
        if ours {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

fn read_instance_file(config_dir: &Path) -> Option<(u16, String)> {
    let text = std::fs::read_to_string(config_dir.join(FILE)).ok()?;
    let mut parts = text.split_whitespace();
    let port = parts.next()?.parse().ok()?;
    let token = parts.next()?.to_owned();
    Some((port, token))
}

/// 128 random bits as hex. `RandomState` is seeded by the system's random
/// source, which spares a dependency for this one use.
fn random_token() -> String {
    let word = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.finish()
    };
    format!(
        "{:016x}{:016x}",
        word(1),
        word(u64::from(std::process::id()))
    )
}

/// Writes `text` to `path`, readable only by its owner on Unix.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("kloggr-instance-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn no_running_instance() {
        let dir = Dir::new("none");
        assert!(!send_to_running(&dir.0, &[]).unwrap());
        // A stale file: nothing listens on its port.
        std::fs::create_dir_all(&dir.0).unwrap();
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        std::fs::write(dir.0.join(FILE), format!("{port} abc\n")).unwrap();
        assert!(!send_to_running(&dir.0, &[]).unwrap());
    }

    #[test]
    fn sends_files_to_the_running_instance() {
        let dir = Dir::new("send");
        let server = std::sync::Arc::new(Server::start(&dir.0).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.0.join(FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let serving = server.clone();
        std::thread::spawn(move || serving.serve(move |files| tx.send(files).unwrap()));

        let relative = PathBuf::from("logs/app.log");
        let absolute = PathBuf::from("/var/log/syslog");
        assert!(send_to_running(&dir.0, &[relative.clone(), absolute.clone()]).unwrap());
        let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            got,
            [std::path::absolute(&relative).unwrap(), absolute.clone()]
        );
        // No files: the running kloggr is just asked to show itself.
        assert!(send_to_running(&dir.0, &[]).unwrap());
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_empty());

        // A wrong token is refused.
        let (port, _) = read_instance_file(&dir.0).unwrap();
        std::fs::write(dir.0.join(FILE), format!("{port} wrong\n")).unwrap();
        // (The answer is an error or no "ok": the connection is dropped.)
        assert!(!matches!(send_to_running(&dir.0, &[absolute]), Ok(true)));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        // Stopping removes the file only if it is still ours.
        server.stop();
        assert!(dir.0.join(FILE).exists(), "another kloggr's file stays");
        write_private(&dir.0.join(FILE), &format!("{port} {}\n", server.token)).unwrap();
        server.stop();
        assert!(!dir.0.join(FILE).exists());
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 32);
    }
}
