//! One app instance per user. A second launch (for example `ternaryshell web-1` from a terminal while
//! the app is open) hands its arguments to the running instance and exits, so two processes
//! never edit the same bookmarks and settings files.
//!
//! The arguments travel over a Unix socket on macOS and Linux. On Windows the running instance
//! listens on a loopback port and writes the port with a random token into the private data
//! folder; a later launch has to present the token, so other users' programs can't send it
//! connections to open.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui;
use parking_lot::Mutex;

use crate::paths;

/// Largest message accepted from another launch.
const MAX_MESSAGE: u64 = 64 * 1024;

pub enum Instance {
    /// This process runs the app. `lock` is held for the process lifetime; `inbox` receives
    /// the arguments of later launches.
    Primary {
        lock: Option<File>,
        inbox: Option<Inbox>,
    },
    /// Another instance is running and took over the arguments.
    Forwarded,
}

fn lock_path() -> PathBuf {
    paths::data_dir().join("instance.lock")
}

/// The socket lives in a private (`0700`) per-user folder: socket paths are limited to about
/// 100 bytes, which a long home folder path can exceed. The name is derived from the data
/// folder, so separate data folders (tests) don't meet.
#[cfg(unix)]
fn socket_dir() -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    // SAFETY: `geteuid` has no preconditions and always returns the process's effective UID.
    let me = unsafe { libc::geteuid() };
    let dir = if cfg!(target_os = "macos") {
        // macOS per-user `$TMPDIR` is already `0700` owned by this user.
        std::env::temp_dir()
    } else if let Some(rt) = dirs::runtime_dir() {
        rt.join("ternaryshell")
    } else {
        std::env::temp_dir().join(format!("ternaryshell-{me}"))
    };
    let mut b = std::fs::DirBuilder::new();
    b.mode(0o700);
    match b.create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() || meta.uid() != me {
        return None;
    }
    if (meta.permissions().mode() & 0o077) != 0 {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some(dir)
}

#[cfg(unix)]
fn socket_path() -> Option<PathBuf> {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in paths::data_dir().as_os_str().as_encoded_bytes() {
        hash = (hash ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    Some(socket_dir()?.join(format!("ternaryshell-{hash:016x}.sock")))
}

/// Checks that the connected Unix socket peer belongs to this user's effective UID.
#[cfg(unix)]
fn peer_is_me(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: `geteuid` is always safe to call.
    let me = unsafe { libc::geteuid() };
    let fd = stream.as_raw_fd();
    #[cfg(target_os = "macos")]
    {
        let mut euid: libc::uid_t = 0;
        let mut egid: libc::gid_t = 0;
        // SAFETY: `fd` is a live Unix domain socket and `euid`/`egid` are valid pointers.
        unsafe { libc::getpeereid(fd, &mut euid, &mut egid) == 0 && euid == me }
    }
    #[cfg(target_os = "linux")]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `fd` is a live Unix socket and `cred`/`len` match `SO_PEERCRED`.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        rc == 0 && cred.uid == me
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (fd, me);
        true
    }
}

/// Becomes the primary instance, or forwards `args` to the running one.
pub fn acquire(args: &[String]) -> Instance {
    let _ = paths::ensure_private_dir(paths::data_dir());
    let lock = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path())
    {
        Ok(f) => f,
        // Without a lock file there is no way to coordinate: just run.
        Err(_) => {
            return Instance::Primary {
                lock: None,
                inbox: None,
            };
        }
    };
    match lock.try_lock() {
        Ok(()) => Instance::Primary {
            lock: Some(lock),
            inbox: listen(),
        },
        Err(TryLockError::WouldBlock) if forward(args) => Instance::Forwarded,
        // The other instance doesn't answer (still starting, or hung): better a second window
        // than a launch that does nothing.
        Err(_) => Instance::Primary {
            lock: None,
            inbox: None,
        },
    }
}

#[cfg(unix)]
fn listen() -> Option<Inbox> {
    use std::os::unix::fs::PermissionsExt;
    let path = socket_path()?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok()?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Some(serve(
        move || loop {
            let (stream, _) = listener.accept()?;
            if peer_is_me(&stream) {
                return Ok(stream);
            }
        },
        |_| true,
    ))
}

/// Where the running instance on Windows leaves its port and token (in the private data folder).
#[cfg(windows)]
fn port_file() -> PathBuf {
    paths::data_dir().join("instance.port")
}

#[cfg(windows)]
const TOKEN_LEN: usize = 64;

#[cfg(windows)]
fn listen() -> Option<Inbox> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let mut raw = [0u8; TOKEN_LEN / 2];
    getrandom::fill(&mut raw).ok()?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    paths::write_private(&port_file(), format!("{port} {token}").as_bytes()).ok()?;
    Some(serve(
        move || listener.accept().map(|(s, _)| s),
        move |data: &mut Vec<u8>| {
            // The message starts with the token; what follows are the arguments.
            if data.len() < TOKEN_LEN || data[..TOKEN_LEN] != *token.as_bytes() {
                return false;
            }
            data.drain(..TOKEN_LEN);
            true
        },
    ))
}

/// Sends `args` to the running instance; true once it confirmed.
#[cfg(windows)]
fn forward(args: &[String]) -> bool {
    let Ok(json) = serde_json::to_vec(args) else {
        return false;
    };
    // The running instance may still be starting up and not listening yet.
    for _ in 0..15 {
        if let Some((port, token)) = std::fs::read_to_string(port_file()).ok().and_then(|s| {
            let (port, token) = s.trim().split_once(' ')?;
            Some((port.parse::<u16>().ok()?, token.to_string()))
        }) && let Ok(mut stream) = std::net::TcpStream::connect_timeout(
            &(std::net::Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_secs(1),
        ) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
            let mut message = token.into_bytes();
            message.extend_from_slice(&json);
            let mut ack = [0u8; 1];
            return stream.write_all(&message).is_ok()
                && stream.shutdown(std::net::Shutdown::Write).is_ok()
                && stream.read_exact(&mut ack).is_ok();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Sends `args` to the running instance; true once it confirmed.
#[cfg(unix)]
fn forward(args: &[String]) -> bool {
    let Ok(message) = serde_json::to_vec(args) else {
        return false;
    };
    // Only a socket created by this user is trusted with the arguments.
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    // SAFETY: `geteuid` is always safe to call.
    let me = unsafe { libc::geteuid() };
    let Some(path) = socket_path() else {
        return false;
    };
    // The running instance may still be starting up and not listening yet.
    for _ in 0..15 {
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if meta.uid() != me || !meta.file_type().is_socket() {
                return false;
            }
            if let Ok(mut stream) = UnixStream::connect(&path) {
                if !peer_is_me(&stream) {
                    return false;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
                let mut ack = [0u8; 1];
                let ok = stream.write_all(&message).is_ok()
                    && stream.shutdown(std::net::Shutdown::Write).is_ok()
                    && stream.read_exact(&mut ack).is_ok();
                return ok;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Arguments of later launches. They are answered from the start, while the window is still
/// being set up (a launch that waits too long for an answer would open a second instance);
/// the app picks them up once it runs.
pub struct Inbox {
    rx: mpsc::Receiver<Vec<String>>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl Inbox {
    /// From now on each later launch brings the window to the front. Returns where their
    /// arguments arrive (including any that came before).
    pub fn attach(self, ctx: &egui::Context) -> mpsc::Receiver<Vec<String>> {
        *self.ctx.lock() = Some(ctx.clone());
        ctx.request_repaint();
        self.rx
    }
}

/// Answers later launches, each a connection `next` waits for; `accept` checks (and may trim)
/// each message before its arguments are taken.
fn serve<S>(
    mut next: impl FnMut() -> std::io::Result<S> + Send + 'static,
    accept: impl Fn(&mut Vec<u8>) -> bool + Send + 'static,
) -> Inbox
where
    S: Read + Write + StreamTimeout,
{
    let (tx, rx) = mpsc::channel();
    let slot: Arc<Mutex<Option<egui::Context>>> = Arc::default();
    let ctx = slot.clone();
    std::thread::Builder::new()
        .name("ternaryshell-instance".into())
        .spawn(move || {
            loop {
                let Ok(mut stream) = next() else {
                    // e.g. out of file descriptors: don't spin.
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let mut data = Vec::new();
                if (&mut stream)
                    .take(MAX_MESSAGE)
                    .read_to_end(&mut data)
                    .is_err()
                {
                    continue;
                }
                if !accept(&mut data) {
                    continue;
                }
                let args: Vec<String> = serde_json::from_slice(&data).unwrap_or_default();
                let _ = stream.write_all(b"1");
                if tx.send(args).is_err() {
                    return;
                }
                if let Some(ctx) = &*ctx.lock() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                }
            }
        })
        .ok();
    Inbox { rx, ctx: slot }
}

/// The read timeout of a stream, for both kinds of connection.
trait StreamTimeout {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
}

#[cfg(unix)]
impl StreamTimeout for UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        UnixStream::set_read_timeout(self, timeout)
    }
}

#[cfg(windows)]
impl StreamTimeout for std::net::TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, timeout)
    }
}

/// The bookmark names and addresses among the process arguments. Options are skipped,
/// including the value of macOS's `-NS…` / `-Apple…` options (e.g. `-NSDocumentRevisions…
/// YES` from Xcode) and the `-psn_…` serial number Finder used to pass.
pub fn targets(args: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg.starts_with("-NS") || arg.starts_with("-Apple") {
            args.next();
        } else if !arg.starts_with('-') && !arg.trim().is_empty() {
            out.push(arg);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::targets;

    #[test]
    fn options_are_not_targets() {
        let args = [
            "-NSDocumentRevisionsDebugMode",
            "YES",
            "-psn_0_1234",
            "web-1",
            "-v",
            "root@db:2222",
        ];
        assert_eq!(
            targets(args.iter().map(|s| s.to_string())),
            ["web-1", "root@db:2222"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn ipc_serve_delivers_args_and_rejects_unaccepted_messages() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::time::Duration;

        let dir = tempfile::Builder::new()
            .prefix("ts-ipc-")
            .tempdir_in("/tmp")
            .unwrap();
        let sock = dir.path().join("i.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let inbox = super::serve(
            move || {
                let (stream, _) = listener.accept()?;
                assert!(super::peer_is_me(&stream));
                Ok(stream)
            },
            |data| {
                if data.starts_with(b"OK:") {
                    data.drain(..3);
                    true
                } else {
                    false
                }
            },
        );
        let ctx = eframe::egui::Context::default();
        let rx = inbox.attach(&ctx);

        // Rejected message gets no ack and is not delivered.
        {
            let mut bad = UnixStream::connect(&sock).unwrap();
            bad.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            bad.write_all(b"NO:[\"evil\"]").unwrap();
            bad.shutdown(std::net::Shutdown::Write).unwrap();
            let mut ack = [0u8; 1];
            assert!(bad.read_exact(&mut ack).is_err());
        }

        // Valid message gets ack and arrives in the inbox.
        {
            let mut good = UnixStream::connect(&sock).unwrap();
            good.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut payload = b"OK:".to_vec();
            payload
                .extend_from_slice(&serde_json::to_vec(&vec!["prod-1", "root@10.0.0.1"]).unwrap());
            good.write_all(&payload).unwrap();
            good.shutdown(std::net::Shutdown::Write).unwrap();
            let mut ack = [0u8; 1];
            good.read_exact(&mut ack).unwrap();
            assert_eq!(&ack, b"1");
        }

        let got = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(got, vec!["prod-1", "root@10.0.0.1"]);
    }
}
