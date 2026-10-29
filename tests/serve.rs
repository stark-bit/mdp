//! Runs the server binary from a shell and checks the CLI/HTTP/WS behavior.

use std::io::{BufRead, BufReader};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use serde_json::Value;
use tokio_tungstenite::tungstenite;

const BIN: &str = env!("CARGO_BIN_EXE_mdp");
const TIMEOUT: Duration = Duration::from_secs(10);
/// Matches the server's fallback range: how many ports after `--port` it
/// tries before giving up.
const PORT_ATTEMPTS: u16 = 20;

/// A running server process, killed on drop.
struct Server {
    child: Child,
    log: PathBuf,
}

fn temp_path(name: &str) -> PathBuf {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "mp-test-{}-{}-{nanos}-{name}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ))
}

impl Server {
    /// Starts the binary with `--no-open`, and returns it along with the
    /// url it printed on stdout. `dir` is the working directory, so the
    /// served root; `files` are the positional file arguments.
    fn start(dir: &Path, files: &[&str]) -> (Server, String) {
        let log = temp_path("mp.log");
        let mut command = Command::new(BIN);
        command
            .env("MP_LOG_FILE", &log)
            .env_remove("MP_LOG_LEVEL")
            .arg("--no-open")
            .args(files)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.starts_with("http://") {
                    let _ = tx.send(line);
                    break;
                }
            }
        });
        let url = rx_url(&rx);
        (Server { child, log }, url)
    }

    fn url_port(url: &str) -> u16 {
        url.strip_prefix("http://localhost:")
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_else(|| panic!("unexpected url {url}"))
            .parse()
            .unwrap_or_else(|_| panic!("unexpected url {url}"))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_file(&self.log);
    }
}

fn rx_url(rx: &std::sync::mpsc::Receiver<String>) -> String {
    rx.recv_timeout(TIMEOUT).expect("the server printed no url")
}

/// A run of `count` ports free on 127.0.0.1 (and IPv6), held until dropped,
/// from below the OS's ephemeral range.
fn hold_ports(count: u16) -> (u16, Vec<TcpListener>) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let seed = std::process::id() as usize + NEXT.fetch_add(1, Ordering::Relaxed) * 997;
    for i in 0..50 {
        let start = 20_000 + ((seed + i * 211) % 10_000) as u16;
        let held: Vec<TcpListener> = (start..start + count)
            .map_while(|p| {
                let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), p);
                if std::net::TcpStream::connect_timeout(&v6, Duration::from_millis(200)).is_ok() {
                    return None;
                }
                TcpListener::bind((Ipv4Addr::LOCALHOST, p)).ok()
            })
            .collect();
        if held.len() == usize::from(count) {
            return (start, held);
        }
    }
    panic!("no run of {count} free ports");
}

fn http_get(url: &str) -> String {
    let port = Server::url_port(url);
    let path = url
        .split_once("localhost:")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, rest)| format!("/{rest}"))
        .unwrap_or_else(|| "/".into());
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    use std::io::Write;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
    response
}

fn serve_cli_and_live_reload() {
    let dir = temp_path("root");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.md");
    std::fs::write(&file, b"# first").unwrap();
    let (mut server, url) = Server::start(&dir, &["a.md"]);
    assert!(url.starts_with("http://localhost:"), "{url}");
    assert!(url.ends_with("/files/a.md"), "{url}");
    let port = Server::url_port(&url);

    let response = http_get(&url);
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: text/html"),
        "{head}"
    );
    assert!(body.contains("<html"), "{body}");

    let ws = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    ws.block_on(async move {
        let ws_url = format!("ws://127.0.0.1:{port}/ws?path=/files/a.md");
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
        let first = next_json(&mut ws).await.unwrap();
        assert_eq!(first["type"], "refresh_content");
        assert_eq!(first["data"]["content"][0], "# first");

        // an edit on disk reaches the page without any editor
        std::fs::write(&file, b"# second").unwrap();
        let second = next_json(&mut ws).await.unwrap();
        assert_eq!(second["type"], "refresh_content");
        assert_eq!(second["data"]["content"][0], "# second");
    });

    server.kill();
    let log = std::fs::read_to_string(&server.log).unwrap();
    assert!(log.contains(&format!("server run: {port}")), "{log}");
}

async fn next_json(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Option<Value> {
    loop {
        match tokio::time::timeout(TIMEOUT, ws.next())
            .await
            .expect("no message")
        {
            Some(Ok(tungstenite::Message::Text(text))) => {
                return Some(serde_json::from_str(&text).unwrap())
            }
            Some(Ok(tungstenite::Message::Close(_))) | Some(Err(_)) | None => return None,
            Some(Ok(_)) => {}
        }
    }
}

#[test]
fn serves_and_live_reloads() {
    serve_cli_and_live_reload();
}

#[test]
fn exits_nonzero_when_the_port_range_is_taken() {
    let (start, _held) = hold_ports(PORT_ATTEMPTS);
    let dir = temp_path("root");
    std::fs::create_dir_all(&dir).unwrap();
    let output = Command::new(BIN)
        .arg("--port")
        .arg(start.to_string())
        .arg("--no-open")
        .arg("a.md")
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to start server"), "{stderr}");
}

#[test]
fn version_matches_cargo_toml() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let version = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap();
    let output = Command::new(BIN).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{version}\n")
    );
}
