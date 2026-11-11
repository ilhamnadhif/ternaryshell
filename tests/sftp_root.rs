//! Root SFTP (through sudo) over a real OpenSSH server, with a stand-in for sudo that runs the
//! command as the same user: passwordless sudo, sudo that wants a password (cached or not),
//! a wrong password, and no password known.

// Against a local OpenSSH server (`/usr/sbin/sshd`): macOS and Linux.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{TestServer, user, wait_for};
use eframe::egui;
use ternaryshell::bookmarks::{Auth, Bookmark};
use ternaryshell::sftp::remote;
use ternaryshell::ssh::session::{ConnectSpec, Connection, RootError, RootLogin};
use ternaryshell::ssh::types::Prompt;

/// The stand-in for sudo. `mode` next to it says what sudo wants: `nopasswd`, `password <pw>`,
/// or `cached <pw>` (once given, the password is remembered for the parent process, as real
/// sudo does without a terminal). Each password line it reads is appended to `read`.
const FAKE_SUDO: &str = r#"#!/bin/sh
dir=$(dirname "$0")
read -r mode want < "$dir/mode"
stdin=0
while [ $# -gt 0 ]; do
    case "$1" in
        -S) stdin=1 ;;
        -p) shift ;;
        --) shift; break ;;
        -*) ;;
        *) break ;;
    esac
    shift
done
stamp="$dir/stamp.$PPID"
case "$mode" in
    nopasswd) exec "$@" ;;
    cached) [ -f "$stamp" ] && exec "$@" ;;
esac
if [ "$stdin" = 1 ]; then
    IFS= read -r pw
    echo "$pw" >> "$dir/read"
    if [ "$pw" = "$want" ]; then
        [ "$mode" = cached ] && : > "$stamp"
        exec "$@"
    fi
    echo "Sorry, try again." >&2
    # Like sudo: another try, from the same input.
    IFS= read -r pw && [ "$pw" = "$want" ] && exec "$@"
    exit 1
fi
echo "sudo: a password is required" >&2
exit 1
"#;

fn set_mode(dir: &Path, mode: &str) {
    std::fs::write(dir.join("mode"), format!("{mode}\n")).unwrap();
    let _ = std::fs::remove_file(dir.join("read"));
}

/// The password lines sudo read since the mode was set.
fn passwords_read(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("read"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn root_sessions_through_sudo() {
    let sudo_dir = tempfile::tempdir().unwrap();
    let sudo = sudo_dir.path().join("sudo");
    std::fs::write(&sudo, FAKE_SUDO).unwrap();
    std::fs::set_permissions(&sudo, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: set before any other thread reads the environment.
    unsafe {
        std::env::set_var("TERNARYSHELL_SECRETS", "memory");
        std::env::set_var("TERNARYSHELL_SUDO", &sudo);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = TestServer::start();
    // A key login: no login password to offer sudo.
    let spec = ConnectSpec {
        bookmark: Bookmark {
            host: "127.0.0.1".into(),
            port: server.port,
            username: user(),
            auth: Auth::Key {
                path: server.key.display().to_string(),
            },
            ..Default::default()
        },
        keepalive_secs: 30,
        jumps: Vec::new(),
        known_hosts: server.dir.path().join("kh"),
        save_secrets: true,
    };
    let conn = Connection::start(rt.handle(), egui::Context::default(), spec);
    wait_for(15, "host key prompt", || {
        match conn.shared.lock().prompt.take() {
            Some(Prompt::HostKey { reply, .. }) => {
                let _ = reply.send(true);
                Some(())
            }
            _ => None,
        }
    });
    wait_for(15, "connected", || {
        conn.state().is_connected().then_some(())
    });
    let opener = conn.sftp_opener();
    let dir = sudo_dir.path();
    let marker = dir.join("visible-to-root");
    std::fs::write(&marker, "x").unwrap();
    let marker_dir = dir.display().to_string();

    // A root session that works: lists a folder.
    let works = |login: RootLogin| {
        let sftp = std::sync::Arc::new(
            rt.block_on(opener.open_root(login))
                .unwrap()
                .unwrap_or_else(|e| panic!("root session: {e:?}")),
        );
        let entries = rt.block_on(remote::list(&sftp, &marker_dir)).unwrap();
        assert!(entries.iter().any(|e| e.name == "visible-to-root"));
    };
    let fails = |login: RootLogin| match rt.block_on(opener.open_root(login)).unwrap() {
        Ok(_) => panic!("root session opened"),
        Err(e) => e,
    };

    // Passwordless sudo: no password asked, none sent.
    set_mode(dir, "nopasswd");
    works(RootLogin::Auto);
    assert!(passwords_read(dir).is_empty());
    // A typed password is not sent when sudo doesn't ask (it would break the SFTP stream).
    works(RootLogin::Password("unused".into()));
    assert!(passwords_read(dir).is_empty());

    // sudo wants a password, and the login was by key: the user is asked.
    set_mode(dir, "password s3cret pass");
    assert_eq!(fails(RootLogin::Auto), RootError::NeedsPassword);
    assert!(passwords_read(dir).is_empty());

    // A wrong one fails at once, without waiting for sudo's next try.
    let started = std::time::Instant::now();
    assert_eq!(
        fails(RootLogin::Password("nope".into())),
        RootError::Denied { login: false }
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(passwords_read(dir), ["nope"]);

    // The right one: checked, then given to the sudo that starts sftp-server. Spaces count.
    set_mode(dir, "password s3cret pass");
    works(RootLogin::Password("s3cret pass".into()));
    assert_eq!(passwords_read(dir), ["s3cret pass", "s3cret pass"]);

    // sudo that remembers the password after the check is not sent it again.
    set_mode(dir, "cached s3cret");
    works(RootLogin::Password("s3cret".into()));
    assert_eq!(passwords_read(dir), ["s3cret"]);

    drop(conn);
}
