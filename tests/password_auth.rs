//! Password and keyboard-interactive login against an in-process russh server (an unprivileged
//! OpenSSH server cannot check passwords, so these flows are tested here), and the login password
//! offered to sudo for root SFTP.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::wait_for;
use eframe::egui;
use russh::server::{Auth as ServerAuth, Msg, Response, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use ternaryshell::bookmarks::{Auth, Bookmark};
use ternaryshell::secrets::{self, SecretKind};
use ternaryshell::ssh::session::{ConnectSpec, Connection, RootError, RootLogin};
use ternaryshell::ssh::types::{ConnError, ConnState, Prompt, SecretAnswer, SecretPromptKind};

#[derive(Clone)]
struct TestServer {
    /// When false only keyboard-interactive is accepted (russh leaves enforcing this to us).
    password_allowed: bool,
    /// What a root SFTP session's sudo was sent (a stand-in for sudo that wants a password and
    /// rejects every one).
    sudo_input: Arc<Mutex<Vec<u8>>>,
}

impl russh::server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        _user: &str,
        password: &str,
    ) -> Result<ServerAuth, Self::Error> {
        // Like OpenSSH: a failed attempt lists the methods that may still succeed.
        if !self.password_allowed {
            return Ok(ServerAuth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::KeyboardInteractive][..])),
                partial_success: false,
            });
        }
        Ok(if password == "secret" {
            ServerAuth::Accept
        } else {
            ServerAuth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::Password][..])),
                partial_success: false,
            }
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &mut self,
        _user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<ServerAuth, Self::Error> {
        Ok(match response {
            None => ServerAuth::Partial {
                name: "".into(),
                instructions: "".into(),
                prompts: vec![
                    ("Password: ".into(), false),
                    ("Verification code: ".into(), true),
                ]
                .into(),
            },
            Some(r) => {
                let answers: Vec<String> = r
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .collect();
                if answers == ["secret", "123456"] {
                    ServerAuth::Accept
                } else {
                    ServerAuth::Reject {
                        proceed_with_methods: None,
                        partial_success: false,
                    }
                }
            }
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, b"welcome-from-test-server\r\n".to_vec())?;
        Ok(())
    }

    /// The root SFTP script: sudo asks for a password.
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, b"ternaryshell-root:password\n".to_vec())?;
        Ok(())
    }

    /// The password line: rejected.
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.sudo_input.lock().unwrap().extend_from_slice(data);
        if data.contains(&b'\n') {
            session.data(channel, b"ternaryshell-root:denied\n".to_vec())?;
            session.eof(channel)?;
            session.close(channel)?;
        }
        Ok(())
    }
}

/// Starts a server offering `methods`; returns its port, and what root SFTP sent sudo.
fn start_server(
    rt: &tokio::runtime::Runtime,
    dir: &std::path::Path,
    methods: &[MethodKind],
) -> (u16, Arc<Mutex<Vec<u8>>>) {
    let key_path = dir.join(format!("host_{}", uuid::Uuid::new_v4()));
    common::keygen(&key_path, "ed25519", "");
    let key =
        russh::keys::decode_secret_key(&std::fs::read_to_string(&key_path).unwrap(), None).unwrap();
    let config = Arc::new(russh::server::Config {
        methods: MethodSet::from(methods),
        keys: vec![key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let password_allowed = methods.contains(&MethodKind::Password);
    let sudo_input = Arc::new(Mutex::new(Vec::new()));
    let server_input = sudo_input.clone();
    rt.spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let config = config.clone();
            let sudo_input = server_input.clone();
            tokio::spawn(async move {
                let handler = TestServer {
                    password_allowed,
                    sudo_input,
                };
                if let Ok(session) = russh::server::run_stream(config, stream, handler).await {
                    let _ = session.await;
                }
            });
        }
    });
    (port, sudo_input)
}

/// Opens a root SFTP session (sudo rejects every password); returns the error and what sudo
/// was sent.
fn try_root(
    rt: &tokio::runtime::Runtime,
    conn: &Connection,
    sudo_input: &Mutex<Vec<u8>>,
    login: RootLogin,
) -> (RootError, String) {
    sudo_input.lock().unwrap().clear();
    let result = rt.block_on(conn.sftp_opener().open_root(login)).unwrap();
    let sent = String::from_utf8(sudo_input.lock().unwrap().clone()).unwrap();
    (result.err().expect("sudo rejects every password"), sent)
}

fn spec(port: u16, id: uuid::Uuid, known: &std::path::Path) -> ConnectSpec {
    ConnectSpec {
        bookmark: Bookmark {
            id,
            host: "127.0.0.1".into(),
            port,
            username: "tester".into(),
            auth: Auth::Password,
            ..Default::default()
        },
        keepalive_secs: 30,
        jumps: Vec::new(),
        known_hosts: known.to_path_buf(),
        save_secrets: true,
    }
}

/// Answers the next prompt; returns (kind, retry, echo) for secrets, or None for host keys.
fn answer(
    conn: &Connection,
    value: Option<&str>,
    save: bool,
) -> Option<(SecretPromptKind, bool, bool)> {
    wait_for(15, "a prompt", || {
        let mut s = conn.shared.lock();
        if let ConnState::Closed(e) = &s.state {
            panic!("connection closed while waiting for a prompt: {e:?}");
        }
        match s.prompt.take() {
            Some(Prompt::HostKey { reply, .. }) => {
                let _ = reply.send(true);
                Some(None)
            }
            Some(Prompt::Secret {
                kind,
                retry,
                echo,
                reply,
                ..
            }) => {
                let _ = reply.send(value.map(|v| SecretAnswer {
                    value: v.into(),
                    save,
                }));
                Some(Some((kind, retry, echo)))
            }
            None => None,
        }
    })
}

#[test]
fn password_and_keyboard_interactive_logins() {
    // SAFETY: set before other threads read the environment.
    unsafe { std::env::set_var("TERNARYSHELL_SECRETS", "memory") };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let known = dir.path().join("known_hosts");
    let ctx = egui::Context::default();

    // --- password only: wrong, then right (saved), then no prompt next time -------------
    let (port, sudo_input) = start_server(&rt, dir.path(), &[MethodKind::Password]);
    let id = uuid::Uuid::new_v4();
    let conn = Connection::start(rt.handle(), ctx.clone(), spec(port, id, &known));
    assert_eq!(answer(&conn, None, false), None, "host key first");
    assert_eq!(
        answer(&conn, Some("wrong"), false),
        Some((SecretPromptKind::Password, false, false))
    );
    assert_eq!(
        answer(&conn, Some("secret"), true),
        Some((SecretPromptKind::Password, true, false))
    );
    wait_for(15, "connected", || {
        let st = conn.state();
        assert!(!st.is_closed(), "{st:?}");
        st.is_connected().then_some(())
    });
    assert_eq!(
        secrets::get(id, SecretKind::Password).unwrap().as_deref(),
        Some("secret")
    );
    // Root SFTP offers sudo the password that logged in (not the rejected one), without asking.
    assert_eq!(
        try_root(&rt, &conn, &sudo_input, RootLogin::Auto),
        (RootError::Denied { login: true }, "secret\n".into())
    );
    // A typed sudo password is sent as typed.
    assert_eq!(
        try_root(
            &rt,
            &conn,
            &sudo_input,
            RootLogin::Password("t yped".into())
        ),
        (RootError::Denied { login: false }, "t yped\n".into())
    );
    drop(conn);

    let conn = Connection::start(rt.handle(), ctx.clone(), spec(port, id, &known));
    wait_for(15, "connected with saved password", || {
        conn.state().is_connected().then_some(())
    });
    assert!(conn.shared.lock().prompt.is_none());
    // The saved password logged in: it is the one offered to sudo.
    assert_eq!(
        try_root(&rt, &conn, &sudo_input, RootLogin::Auto),
        (RootError::Denied { login: true }, "secret\n".into())
    );
    drop(conn);

    // Three wrong answers end with a clear error.
    let id2 = uuid::Uuid::new_v4();
    let conn = Connection::start(rt.handle(), ctx.clone(), spec(port, id2, &known));
    for _ in 0..3 {
        answer(&conn, Some("nope"), false);
    }
    let st = wait_for(15, "auth failure", || {
        conn.state().is_closed().then(|| conn.state())
    });
    assert!(
        matches!(st, ConnState::Closed(Some(ConnError::AuthFailed(_)))),
        "{st:?}"
    );

    // --- keyboard-interactive (PAM style): password filled in, one-time code asked -----
    let (port, sudo_input) = start_server(&rt, dir.path(), &[MethodKind::KeyboardInteractive]);
    let conn = Connection::start(
        rt.handle(),
        ctx.clone(),
        spec(port, uuid::Uuid::new_v4(), &known),
    );
    assert_eq!(answer(&conn, None, false), None, "host key first");
    assert_eq!(
        answer(&conn, Some("secret"), false),
        Some((SecretPromptKind::Password, false, false))
    );
    assert_eq!(
        answer(&conn, Some("123456"), false),
        Some((SecretPromptKind::Interactive, false, true))
    );
    wait_for(15, "connected via keyboard-interactive", || {
        conn.state().is_connected().then_some(())
    });
    // The password answered the server's password prompt: it is offered to sudo too.
    assert_eq!(
        try_root(&rt, &conn, &sudo_input, RootLogin::Auto),
        (RootError::Denied { login: true }, "secret\n".into())
    );
}
