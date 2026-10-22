// the port lock is held across awaits on purpose, see lock_ports
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::config::{parse_port, truthy, var_string, Config};
use crate::editor::tests::FakeEditor;
use axum::body::to_bytes;
use futures_util::StreamExt;
use std::net::TcpListener as StdListener;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;
use tokio_tungstenite::tungstenite;

const TIMEOUT: Duration = Duration::from_secs(5);

/// A directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!(
            "mkdp-test-{}-{}-{nanos}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // canonical, so paths compare equal on macOS where /var -> /private/var
        TempDir(dir.canonicalize().unwrap())
    }

    /// Creates `relative` (and its parents) with `contents`.
    fn file(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An editor that records the functions the server calls (browser opening,
/// url echo, hooks) and answers everything with null. Preview content comes
/// from disk now, so the tests need no editor behavior.
fn fake() -> FakeEditor {
    FakeEditor::new(|_| Ok(Value::Null))
}

fn test_app(config: Config) -> Arc<App> {
    App::new(config, fake().editor.clone(), 4321, None)
}

/// A Config with the values the tests used to set as editor vars.
fn config_from(vars: Value) -> Config {
    let mut config = Config::default();
    if let Some(v) = vars.get("mkdp_open_to_the_world") {
        config.open_to_the_world = truthy(v);
    }
    if let Some(v) = vars.get("mkdp_port") {
        config.port = parse_port(v);
    }
    if let Some(v) = vars.get("mkdp_browserfunc") {
        config.browserfunc = Some(var_string(v.clone())).filter(|f| !f.is_empty());
    }
    if let Some(v) = vars.get("mkdp_open_ip") {
        config.open_ip = Some(var_string(v.clone())).filter(|f| !f.is_empty());
    }
    if let Some(v) = vars.get("mkdp_echo_preview_url") {
        config.echo_preview_url = truthy(v);
    }
    if let Some(v) = vars.get("mkdp_images_path") {
        config.images_path =
            Some(PathBuf::from(var_string(v.clone()))).filter(|p| !p.as_os_str().is_empty());
    }
    if let Some(v) = vars.get("mkdp_markdown_css") {
        config.markdown_css =
            Some(PathBuf::from(var_string(v.clone()))).filter(|p| !p.as_os_str().is_empty());
    }
    if let Some(v) = vars.get("mkdp_highlight_css") {
        config.highlight_css =
            Some(PathBuf::from(var_string(v.clone()))).filter(|p| !p.as_os_str().is_empty());
    }
    if let Some(v) = vars.get("mkdp_theme_css") {
        config.theme_css =
            Some(PathBuf::from(var_string(v.clone()))).filter(|p| !p.as_os_str().is_empty());
    }
    config
}

async fn get(app: &Arc<App>, path: &str, referer: Option<&str>) -> (StatusCode, String, Vec<u8>) {
    let mut req = Request::builder().uri(path);
    if let Some(referer) = referer {
        req = req.header(header::REFERER, referer);
    }
    let res = route(State(app.clone()), req.body(Body::empty()).unwrap()).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    let body = to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, content_type, body)
}

fn embedded_file(path: &str) -> Vec<u8> {
    OUT.get_file(path).unwrap().contents().to_vec()
}

/// Polls `check` until it returns something.
async fn wait_for<T>(mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A port that is free on 127.0.0.1 along with the `count - 1` after it.
/// Picked below the OS's ephemeral range, so tests binding port 0 in
/// parallel don't take it.
/// Held by the tests that bind ports, which would otherwise walk into each
/// other's ports when bind() falls back to the next one, and by tests that
/// spawn processes: a child holds the test process's sockets until it execs.
static PORTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn lock_ports() -> std::sync::MutexGuard<'static, ()> {
    PORTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A run of `count` ports free on 127.0.0.1 (and IPv6), held until dropped.
/// Picked below the OS's ephemeral range, so tests binding port 0 in
/// parallel don't take them.
fn hold_ports(count: u16) -> (u16, Vec<StdListener>) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let seed = std::process::id() as usize + NEXT.fetch_add(1, Ordering::Relaxed) * 997;
    for i in 0..50 {
        let start = 20_000 + ((seed + i * 211) % 10_000) as u16;
        let held: Vec<StdListener> = (start..start + count)
            .map_while(|p| {
                if ipv6_taken(p) {
                    return None;
                }
                StdListener::bind(("127.0.0.1", p)).ok()
            })
            .collect();
        if held.len() == usize::from(count) {
            return (start, held);
        }
    }
    panic!("no run of {count} free ports");
}

/// Like [`hold_ports`], but released for the test to bind.
fn free_ports(count: u16) -> u16 {
    let (start, held) = hold_ports(count);
    drop(held);
    start
}

#[tokio::test]
async fn bind_uses_the_preferred_port() {
    let _ports = lock_ports();
    let port = free_ports(1);
    let listener = bind(false, Some(port)).await.unwrap();
    assert_eq!(listener.local_addr().unwrap().port(), port);
}

#[tokio::test]
async fn bind_falls_back_to_the_next_port() {
    let _ports = lock_ports();
    let port = free_ports(2);
    let _taken = StdListener::bind(("127.0.0.1", port)).unwrap();
    let listener = bind(false, Some(port)).await.unwrap();
    assert_eq!(listener.local_addr().unwrap().port(), port + 1);
}

#[tokio::test]
async fn bind_skips_a_port_another_program_has_on_ipv6() {
    let _ports = lock_ports();
    let port = free_ports(2);
    // what Node.js and most servers do by default
    let Ok(_taken) = StdListener::bind(("::", port)) else {
        return; // no IPv6 on this machine
    };
    let listener = bind(false, Some(port)).await.unwrap();
    assert_eq!(listener.local_addr().unwrap().port(), port + 1);
}

#[tokio::test]
async fn bind_lets_the_os_pick_when_every_attempt_is_taken() {
    let _ports = lock_ports();
    let (start, _taken) = hold_ports(PORT_ATTEMPTS);
    let listener = bind(false, Some(start)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    assert_ne!(port, 0);
    assert!(!(start..start + PORT_ATTEMPTS).contains(&port), "{port}");
}

#[tokio::test]
async fn bind_without_a_preferred_port() {
    let _ports = lock_ports();
    let listener = bind(false, None).await.unwrap();
    assert_ne!(listener.local_addr().unwrap().port(), 0);
}

#[tokio::test]
async fn bind_is_local_unless_open_to_the_world() {
    let _ports = lock_ports();
    let local = bind(false, None).await.unwrap().local_addr().unwrap();
    assert_eq!(local.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    let world = bind(true, None).await.unwrap().local_addr().unwrap();
    assert_eq!(world.ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
}

#[tokio::test]
async fn pages_serve_the_index() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;
    let index = embedded_file("index.html");
    for path in ["/", "/files/docs/a.md", "/files/docs/a.md?x=1"] {
        let (status, content_type, body) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(
            content_type.starts_with("text/html"),
            "{path}: {content_type}"
        );
        assert_eq!(body, index, "{path}");
    }
    let (status, _, body) = get(&app, "/files/docs/missing.md", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_ne!(body, index);
}

#[tokio::test]
async fn unknown_paths_are_404() {
    let app = test_app(Config::default());
    let (status, content_type, body) = get(&app, "/no/such/file.js", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(content_type.starts_with("text/html"), "{content_type}");
    assert_eq!(body, embedded_file("404.html"));
}

#[tokio::test]
async fn embedded_assets_are_served() {
    let app = test_app(Config::default());
    let (status, content_type, body) = get(&app, "/_static/favicon.ico", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("image/"), "{content_type}");
    assert_eq!(body, embedded_file("_static/favicon.ico"));
}

#[tokio::test]
async fn custom_css_replaces_the_embedded_css() {
    let tmp = TempDir::new();
    let markdown = tmp.file("my markdown.css", b"body { color: red }");
    let highlight = tmp.file("hl.css", b".hljs { color: blue }");
    let app = test_app(config_from(json!({
        "mkdp_markdown_css": markdown.to_string_lossy(),
        "mkdp_highlight_css": highlight.to_string_lossy(),
    })));
    for (path, expected) in [
        ("/_static/markdown.css", "body { color: red }"),
        ("/_static/highlight.css", ".hljs { color: blue }"),
    ] {
        let (status, content_type, body) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(content_type, "text/css", "{path}");
        assert_eq!(String::from_utf8(body).unwrap(), expected, "{path}");
    }
}

#[tokio::test]
async fn css_falls_back_to_the_embedded_file() {
    let tmp = TempDir::new();
    // unset, and set to a file that does not exist
    let app = test_app(config_from(
        json!({ "mkdp_highlight_css": tmp.0.join("missing.css").to_string_lossy() }),
    ));
    for name in ["markdown.css", "highlight.css"] {
        let (status, content_type, body) = get(&app, &format!("/_static/{name}"), None).await;
        assert_eq!(status, StatusCode::OK, "{name}");
        assert_eq!(content_type, "text/css", "{name}");
        assert_eq!(body, embedded_file(&format!("_static/{name}")), "{name}");
    }
}

#[test]
fn theme_css_keeps_a_shadcn_v4_theme() {
    let css = ":root {\n  --radius: 0.625rem;\n  --background: oklch(1 0 0);\n  --font-sans: Inter, sans-serif;\n}\n.dark {\n  --border: oklch(1 0 0 / 10%);\n}\n@theme inline {\n  --color-background: var(--background);\n}";
    assert_eq!(theme_css(css), css);
}

#[test]
fn theme_css_wraps_tailwind_v3_hsl_channels() {
    let css = "@layer base {\n  :root {\n    --background: 0 0% 100%;\n    --primary:222.2 47.4% 11.2%;\n    --ring: 215deg 20.2% 65.1% / 50%;\n    --radius: 0.5rem;\n  }\n  .dark { --muted: 217.2 32.6% 17.5% }\n}";
    assert_eq!(
        theme_css(css),
        "@layer base {\n  :root {\n    --background: hsl(0 0% 100%);\n    --primary: hsl(222.2 47.4% 11.2%);\n    --ring: hsl(215deg 20.2% 65.1% / 50%);\n    --radius: 0.5rem;\n  }\n  .dark { --muted: hsl(217.2 32.6% 17.5%)}\n}"
    );
}

#[test]
fn theme_css_drops_package_imports_only() {
    let css = "@import \"tailwindcss\";\n@import 'tw-animate-css';\n@import url(\"https://fonts.googleapis.com/css2?family=Inter\");\n@import \"./fonts.css\";\n@import \"https://example.com/x\";\n:root { --x: 1px; }";
    assert_eq!(
        theme_css(css),
        "@import url(\"https://fonts.googleapis.com/css2?family=Inter\");\n@import \"./fonts.css\";\n@import \"https://example.com/x\";\n:root { --x: 1px; }"
    );
}

#[tokio::test]
async fn theme_css_is_empty_without_a_theme() {
    let tmp = TempDir::new();
    for vars in [
        json!({}),
        json!({ "mkdp_theme_css": tmp.0.join("missing.css").to_string_lossy() }),
    ] {
        let app = test_app(config_from(vars.clone()));
        let (status, content_type, body) = get(&app, "/_theme/theme.css", None).await;
        assert_eq!(status, StatusCode::OK, "{vars}");
        assert_eq!(content_type, "text/css", "{vars}");
        assert!(body.is_empty(), "{vars}");
    }
}

#[tokio::test]
async fn theme_css_and_its_fonts_are_served() {
    let tmp = TempDir::new();
    let theme = tmp.file("theme/globals.css", b":root { --primary: 0 0% 9%; }");
    tmp.file("theme/fonts/My Font.woff2", b"woff2 bytes");
    tmp.file("theme/notes.txt", b"secret");
    tmp.file("outside.woff2", b"outside");
    let app = test_app(config_from(
        json!({ "mkdp_theme_css": theme.to_string_lossy() }),
    ));

    let (status, content_type, body) = get(&app, "/_theme/theme.css", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/css");
    assert_eq!(body, b":root { --primary: hsl(0 0% 9%); }");

    let (status, content_type, body) = get(&app, "/_theme/fonts/My%20Font.woff2", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "font/woff2");
    assert_eq!(body, b"woff2 bytes");

    // only fonts, and only from the theme's folder
    for path in [
        "/_theme/notes.txt",
        "/_theme/globals.css",
        "/_theme/../outside.woff2",
        "/_theme/%2e%2e/outside.woff2",
        "/_theme/missing.woff2",
    ] {
        let (status, _, body) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_ne!(body, b"secret", "{path}");
        assert_ne!(body, b"outside", "{path}");
    }
}

#[tokio::test]
async fn local_image_is_relative_to_the_file() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("docs/img/a.png", b"png bytes");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;

    let referer = Some("http://localhost:4321/files/docs/a.md");
    let (status, content_type, body) = get(&app, "/_assets/img/a.png", referer).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "image/png");
    assert_eq!(body, b"png bytes");

    // the page's query string and hash are not part of the file path
    let referer = Some("http://localhost:4321/files/docs/a.md?theme=dark#top");
    let (status, _, body) = get(&app, "/_assets/img/a.png", referer).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"png bytes");
}

#[tokio::test]
async fn local_image_uses_the_images_path() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("assets/a.png", b"from images path");
    let app = serve(
        config_from(json!({
            "mkdp_images_path": tmp.0.join("assets").to_string_lossy(),
        })),
        Some(&tmp.0),
    )
    .await
    .0;
    let (status, _, body) = get(
        &app,
        "/_assets/a.png",
        Some("http://localhost:4321/files/docs/a.md"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"from images path");
}

#[tokio::test]
async fn local_image_decodes_the_path() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("docs/my pic.jpg", b"jpeg bytes");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;
    for path in [
        "/_assets/my%20pic.jpg",
        // encoded by the page and again by the browser
        "/_assets/my%2520pic.jpg",
        // `my\ pic.jpg`, a shell-escaped space
        "/_assets/my%5C%20pic.jpg",
    ] {
        let (status, content_type, body) =
            get(&app, path, Some("http://localhost:4321/files/docs/a.md")).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(content_type, "image/jpeg", "{path}");
        assert_eq!(body, b"jpeg bytes", "{path}");
    }
}

#[tokio::test]
async fn local_image_rooted_path_is_found_in_an_ancestor() {
    let tmp = TempDir::new();
    tmp.file("project/docs/sub/a.md", b"# a");
    tmp.file("project/images/mkdp-test-logo.svg", b"<svg/>");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;
    let (status, content_type, body) = get(
        &app,
        "/_assets//images/mkdp-test-logo.svg",
        Some("http://localhost:4321/files/project/docs/sub/a.md"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "image/svg+xml");
    assert_eq!(body, b"<svg/>");
}

#[tokio::test]
async fn local_image_absolute_path_inside_the_file_dir() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    let image = tmp.file("docs/a.gif", b"gif bytes");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;
    let path = format!("/_assets/{}", image.to_string_lossy());
    let (status, content_type, body) =
        get(&app, &path, Some("http://localhost:4321/files/docs/a.md")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "image/gif");
    assert_eq!(body, b"gif bytes");
}

#[tokio::test]
async fn local_image_needs_a_resolvable_page() {
    let tmp = TempDir::new();
    tmp.file("docs/a.png", b"png bytes");
    tmp.file("docs/a.md", b"# a");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;

    let (status, _, _) = get(
        &app,
        "/_assets/a.png",
        Some("http://localhost:4321/files/docs/a.md"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "control request");

    let (status, _, body) = get(
        &app,
        "/_assets/a.png",
        Some("http://localhost:4321/files/other.md"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, embedded_file("404.html"));
    let (status, _, _) = get(&app, "/_assets/a.png", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(&app, "/_assets/a.png", Some("http://localhost:4321/")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn local_image_missing_file_is_404() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("docs/a.png", b"png bytes");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;
    let referer = Some("http://localhost:4321/files/docs/a.md");
    let (status, _, _) = get(&app, "/_assets/a.png", referer).await;
    assert_eq!(status, StatusCode::OK, "control request");
    let (status, _, body) = get(&app, "/_assets/b.png", referer).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, embedded_file("404.html"));
    // a directory is not an image
    let (status, _, _) = get(&app, "/_assets/.", referer).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn local_image_only_serves_images() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("docs/a.png", b"png bytes");
    let secret = TempDir::new();
    let secret_file = secret.file("id_rsa", b"PRIVATE KEY");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;
    let referer = Some("http://localhost:4321/files/docs/a.md");

    let (status, _, _) = get(&app, "/_assets/a.png", referer).await;
    assert_eq!(status, StatusCode::OK, "control request");

    let path = format!("/_assets/{}", secret_file.to_string_lossy());
    let (status, _, body) = get(&app, &path, referer).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{path} was served");
    assert_ne!(body, b"PRIVATE KEY");

    let (status, _, body) = get(&app, "/_assets//etc/passwd", referer).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "/etc/passwd was served");
    assert!(!String::from_utf8_lossy(&body).contains("root:"));
}

#[tokio::test]
async fn local_image_does_not_traverse_to_non_images() {
    let tmp = TempDir::new();
    tmp.file("project/docs/a.md", b"# a");
    tmp.file("project/images/a.png", b"png bytes");
    tmp.file("secret.txt", b"TOKEN=hunter2");
    let app = serve(Config::default(), Some(&tmp.0)).await.0;
    let referer = Some("http://localhost:4321/files/project/docs/a.md");

    // `../images/a.png` is an ordinary markdown image link
    let (status, _, body) = get(&app, "/_assets/../images/a.png", referer).await;
    assert_eq!(status, StatusCode::OK, "control request");
    assert_eq!(body, b"png bytes");

    for path in [
        "/_assets/../../secret.txt",
        "/_assets/%2E%2E%2F%2E%2E%2Fsecret.txt",
    ] {
        let (status, _, body) = get(&app, path, referer).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} was served");
        assert_ne!(body, b"TOKEN=hunter2");
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Serves `config` with `root` as the served root, plus its fake editor.
async fn serve(config: Config, root: Option<&Path>) -> (Arc<App>, FakeEditor) {
    let editor = fake();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = App::new(
        config,
        editor.editor.clone(),
        port,
        root.map(Path::to_path_buf),
    );
    let router = router(app.clone());
    tokio::spawn(async move { axum::serve(listener, router).await });
    (app, editor)
}

/// Connects the page at url path `page` and returns it and its initial data.
async fn connect_page(app: &App, page: &str) -> (Ws, Value) {
    let path = utf8_percent_encode(page, NON_ALPHANUMERIC);
    let url = format!("ws://127.0.0.1:{}/ws?path={path}", app.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let first = next_json(&mut ws).await.unwrap();
    assert_eq!(first["type"], "refresh_content");
    (ws, first["data"].clone())
}

/// The next message on `ws`, or `None` once it is closed.
async fn next_json(ws: &mut Ws) -> Option<Value> {
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

fn notifier(app: &Arc<App>) -> mpsc::UnboundedSender<(String, Value)> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(app.clone().process_notifications(rx));
    tx
}

#[tokio::test]
async fn websocket_needs_a_page() {
    let (app, _) = serve(Config::default(), None).await;
    let url = format!("ws://127.0.0.1:{}/ws", app.port);
    match tokio_tungstenite::connect_async(url).await {
        Err(tungstenite::Error::Http(res)) => assert_eq!(res.status(), StatusCode::BAD_REQUEST),
        other => panic!("expected a 400, got {other:?}"),
    }
}

#[tokio::test]
async fn websocket_rejects_other_web_sites() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    let (app, _) = serve(Config::default(), Some(&tmp.0)).await;
    let url = format!("ws://127.0.0.1:{}/ws?path=/files/docs/a.md", app.port);
    let mut req = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    req.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("https://evil.example"),
    );
    match tokio_tungstenite::connect_async(req).await {
        Err(tungstenite::Error::Http(res)) => assert_eq!(res.status(), StatusCode::FORBIDDEN),
        other => panic!("expected a 403, got {other:?}"),
    }
}

#[tokio::test]
async fn websocket_accepts_the_page_origin() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    let (app, _) = serve(Config::default(), Some(&tmp.0)).await;
    let url = format!("ws://127.0.0.1:{}/ws?path=/files/docs/a.md", app.port);
    let mut req = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    let origin = format!("http://127.0.0.1:{}", app.port);
    req.headers_mut()
        .insert(header::ORIGIN, HeaderValue::from_str(&origin).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(next_json(&mut ws).await.unwrap()["type"], "refresh_content");
}

#[tokio::test]
async fn open_to_the_world_needs_the_token() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("docs/a.png", b"png bytes");
    let (app, _) = serve(
        config_from(json!({ "mkdp_open_to_the_world": 1 })),
        Some(&tmp.0),
    )
    .await;
    let token = app.token.clone();
    assert_eq!(token.len(), 32);
    assert_ne!(
        token,
        App::new(Config::default(), fake().editor.clone(), 1, None).token
    );

    for path in [
        "/files/docs/a.md",
        "/",
        "/_assets/a.png",
        "/_theme/theme.css",
    ] {
        let (status, _, body) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(!body.contains(&b'<'), "{path} served the page");
        let (status, _, _) = get(&app, &format!("{path}?token=wrong"), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{path} with a wrong token"
        );
    }
    // the page's scripts need no token
    let (status, _, _) = get(&app, "/404.html", None).await;
    assert_eq!(status, StatusCode::OK);

    // the link the server opens works and hands the page a cookie
    let req = Request::builder()
        .uri(format!("/files/docs/a.md?token={token}"))
        .body(Body::empty())
        .unwrap();
    let res = route(State(app.clone()), req).await;
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        cookie.starts_with(&format!("mkdp_token_{}={token};", app.port)),
        "{cookie}"
    );
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"),
        "{cookie}"
    );

    // which images and the socket then accept
    let req = Request::builder()
        .uri("/_assets/a.png")
        .header(header::REFERER, "http://localhost/files/docs/a.md")
        .header(
            header::COOKIE,
            format!("other=1; mkdp_token_{}={token}", app.port),
        )
        .body(Body::empty())
        .unwrap();
    let res = route(State(app.clone()), req).await;
    assert_eq!(res.status(), StatusCode::OK);

    let url = format!("ws://127.0.0.1:{}/ws?path=/files/docs/a.md", app.port);
    match tokio_tungstenite::connect_async(url.clone()).await {
        Err(tungstenite::Error::Http(res)) => assert_eq!(res.status(), StatusCode::UNAUTHORIZED),
        other => panic!("expected a 401, got {other:?}"),
    }
    let mut req = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
    req.headers_mut().insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("mkdp_token_{}={token}", app.port)).unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(next_json(&mut ws).await.unwrap()["type"], "refresh_content");
}

#[tokio::test]
async fn open_file_calls_browserfunc_with_the_page_url() {
    let tmp = TempDir::new();
    let file = tmp.file("docs/a.md", b"# a");
    let (app, editor) = serve(
        config_from(json!({ "mkdp_browserfunc": "OpenPreview" })),
        Some(&tmp.0),
    )
    .await;
    notifier(&app)
        .send(("open_browser".into(), json!({ "path": file })))
        .unwrap();
    let opened =
        wait_for(|| Some(editor.function_calls("OpenPreview")).filter(|c| !c.is_empty())).await;
    assert_eq!(
        opened,
        vec![vec![json!(format!(
            "http://localhost:{}/files/docs/a.md",
            app.port
        ))]]
    );
    assert!(editor.function_calls("mkdp#util#echo_url").is_empty());
}

#[tokio::test]
async fn open_file_uses_the_open_ip_and_echoes_the_url() {
    let tmp = TempDir::new();
    let file = tmp.file("docs/a.md", b"# a");
    let (app, editor) = serve(
        config_from(json!({
            "mkdp_browserfunc": "OpenPreview",
            "mkdp_open_ip": "192.0.2.7",
            "mkdp_echo_preview_url": 1,
            "mkdp_open_to_the_world": 1,
        })),
        Some(&tmp.0),
    )
    .await;
    notifier(&app)
        .send(("open_browser".into(), json!({ "path": file })))
        .unwrap();
    let url = json!(format!(
        "http://192.0.2.7:{}/files/docs/a.md?token={}",
        app.port, app.token
    ));
    let echoed =
        wait_for(|| Some(editor.function_calls("mkdp#util#echo_url")).filter(|c| !c.is_empty()))
            .await;
    assert_eq!(echoed, vec![vec![url.clone()]]);
    assert_eq!(editor.function_calls("OpenPreview"), vec![vec![url]]);
}

#[tokio::test]
async fn close_page_only_closes_that_files_pages() {
    let tmp = TempDir::new();
    let a = tmp.file("docs/a.md", b"# a");
    let b = tmp.file("docs/b.md", b"# b");
    let (app, _) = serve(Config::default(), Some(&tmp.0)).await;
    let (mut page1, _) = connect_page(&app, "/files/docs/a.md").await;
    let (mut page2, _) = connect_page(&app, "/files/docs/a.md").await;
    let (mut page3, _) = connect_page(&app, "/files/docs/b.md").await;

    let notify = notifier(&app);
    notify
        .send(("close_page".into(), json!([{ "path": a }])))
        .unwrap();
    for page in [&mut page1, &mut page2] {
        assert_eq!(next_json(page).await, Some(json!({ "type": "close_page" })));
        assert_eq!(next_json(page).await, None, "page should be disconnected");
    }

    // the other file's page still gets updates, and nothing before them
    notify
        .send(("refresh_content".into(), json!({ "path": b })))
        .unwrap();
    let next = next_json(&mut page3).await.unwrap();
    assert_eq!(next["type"], "refresh_content");
    assert_eq!(next["data"]["content"][0], "# b");

    notify
        .send(("close_page".into(), json!([{ "path": b }])))
        .unwrap();
    assert_eq!(
        next_json(&mut page3).await,
        Some(json!({ "type": "close_page" }))
    );
}

#[tokio::test]
async fn close_all_pages_closes_every_page() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("docs/b.md", b"# b");
    let (app, _) = serve(Config::default(), Some(&tmp.0)).await;
    let (mut page1, _) = connect_page(&app, "/files/docs/a.md").await;
    let (mut page2, _) = connect_page(&app, "/files/docs/b.md").await;
    app.close_all_pages().await;
    assert_eq!(app.connected.load(Ordering::SeqCst), 0);
    for page in [&mut page1, &mut page2] {
        assert_eq!(next_json(page).await, Some(json!({ "type": "close_page" })));
        assert_eq!(next_json(page).await, None);
    }
}

#[tokio::test]
async fn repeated_refreshes_are_collapsed() {
    let tmp = TempDir::new();
    let a = tmp.file("docs/a.md", b"# a");
    let b = tmp.file("docs/b.md", b"# b");
    let (app, _) = serve(Config::default(), Some(&tmp.0)).await;
    let (mut page_a, _) = connect_page(&app, "/files/docs/a.md").await;
    let (mut page_b, _) = connect_page(&app, "/files/docs/b.md").await;

    // queued before processing starts, so they are handled as one batch
    let (tx, rx) = mpsc::unbounded_channel();
    let refresh = |path: &Path| ("refresh_content".to_string(), json!([{ "path": path }]));
    for msg in [
        refresh(&a),
        refresh(&a),
        refresh(&a),
        refresh(&b),
        refresh(&a),
    ] {
        tx.send(msg).unwrap();
    }
    tokio::spawn(app.clone().process_notifications(rx));

    // the three consecutive refreshes of a collapse into one
    assert_eq!(
        next_json(&mut page_a).await.unwrap()["data"]["content"][0],
        "# a"
    );
    assert_eq!(
        next_json(&mut page_b).await.unwrap()["data"]["content"][0],
        "# b"
    );
    assert_eq!(
        next_json(&mut page_a).await.unwrap()["data"]["content"][0],
        "# a"
    );
    // and there is no fourth push for a
    match tokio::time::timeout(Duration::from_millis(300), page_a.next()).await {
        Err(_) => {} // nothing extra arrived
        other => panic!("unexpected extra message: {other:?}"),
    }
}

#[tokio::test]
async fn files_page_reads_the_file_from_disk() {
    let tmp = TempDir::new();
    let file = tmp.file("docs/my file.md", b"# a\n\nsecond line");
    let (app, editor) = serve(Config::default(), Some(&tmp.0)).await;

    let (mut page, data) = connect_page(&app, "/files/docs/my%20file.md").await;
    assert_eq!(data["content"][0], "# a");
    assert_eq!(data["content"][2], "second line");
    assert_eq!(data["name"], json!(file.to_string_lossy()));
    assert_eq!(data["isActive"], 0);
    assert_eq!(data["cursor"], json!([0, 0, 0, 0]));
    assert!(editor
        .function_calls("mkdp#util#file_preview_data")
        .is_empty());

    // and follows its edits
    std::fs::write(&file, b"# changed").unwrap();
    notifier(&app)
        .send(("refresh_content".into(), json!([{ "path": file }])))
        .unwrap();
    assert_eq!(
        next_json(&mut page).await.unwrap()["data"]["content"][0],
        "# changed"
    );
}

#[tokio::test]
async fn files_urls_serve_markdown_under_the_root() {
    let tmp = TempDir::new();
    tmp.file("docs/a.md", b"# a");
    tmp.file("docs/my file.md", b"# spaces");
    tmp.file("docs/pic.png", b"png bytes");
    tmp.file("notes.txt", b"not markdown");
    tmp.file(".env", b"TOKEN=hunter2");
    let (app, _) = serve(Config::default(), Some(&tmp.0)).await;
    let index = embedded_file("index.html");

    for path in ["/files/docs/a.md", "/files/docs/my%20file.md"] {
        let (status, _, body) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body, index, "{path}");
    }
    // an image a page links to
    let (status, content_type, body) = get(&app, "/files/docs/pic.png", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "image/png");
    assert_eq!(body, b"png bytes");

    for path in [
        "/files/notes.txt",
        "/files/.env",
        "/files/docs/missing.md",
        "/files/docs",
        "/files/",
    ] {
        let (status, _, body) = get(&app, path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(body, embedded_file("404.html"), "{path}");
    }
}

#[tokio::test]
async fn files_urls_cannot_leave_the_root() {
    let tmp = TempDir::new();
    let root = tmp.dir("root");
    tmp.file("root/docs/a.md", b"# a");
    let outside = tmp.file("outside.md", b"# outside");
    tmp.file("outside.png", b"png bytes");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, root.join("link.md")).unwrap();
        std::os::unix::fs::symlink(tmp.0.join("outside.png"), root.join("link.png")).unwrap();
    }
    let (app, editor) = serve(Config::default(), Some(&root)).await;

    let (status, _, _) = get(&app, "/files/docs/a.md", None).await;
    assert_eq!(status, StatusCode::OK, "control request");

    let absolute = utf8_percent_encode(&outside.to_string_lossy(), NON_ALPHANUMERIC).to_string();
    for path in [
        "/files/../outside.md".to_string(),
        "/files/docs/../../outside.md".to_string(),
        "/files/%2E%2E/outside.md".to_string(),
        "/files/docs%2F..%2F..%2Foutside.md".to_string(),
        "/files/..%5Coutside.md".to_string(),
        "/files/../outside.png".to_string(),
        format!("/files/{absolute}"),
        format!("/files/{}", outside.to_string_lossy()),
        "/files/link.md".to_string(),
        "/files/link.png".to_string(),
    ] {
        let (status, _, body) = get(&app, &path, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} was served");
        assert_ne!(body, b"png bytes", "{path}");

        let page = utf8_percent_encode(&path, NON_ALPHANUMERIC);
        let url = format!("ws://127.0.0.1:{}/ws?path={page}", app.port);
        match tokio_tungstenite::connect_async(url).await {
            Err(tungstenite::Error::Http(res)) => {
                assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{path}")
            }
            other => panic!("{path}: expected a 400, got {other:?}"),
        }
    }
    assert!(editor
        .function_calls("mkdp#util#file_preview_data")
        .is_empty());
}

#[tokio::test]
async fn open_file_urls_under_and_outside_the_root() {
    let tmp = TempDir::new();
    let root = tmp.dir("root");
    let inside = tmp.file("root/docs/my file.md", b"# a");
    let outside = tmp.file("elsewhere/b.md", b"# b");
    let (app, editor) = serve(
        config_from(json!({ "mkdp_browserfunc": "OpenPreview" })),
        Some(&root),
    )
    .await;
    let notify = notifier(&app);
    notify
        .send(("open_browser".into(), json!({ "path": inside })))
        .unwrap();
    notify
        .send(("open_browser".into(), json!({ "path": outside })))
        .unwrap();
    let opened =
        wait_for(|| Some(editor.function_calls("OpenPreview")).filter(|c| c.len() == 2)).await;
    let url = |path: &str| vec![json!(format!("http://localhost:{}{path}", app.port))];
    assert_eq!(
        opened,
        vec![
            // under the root: the /files/ url
            url("/files/docs/my%20file.md"),
            // outside the root: the raw canonical path
            url(&format!("{}", outside.to_string_lossy())),
        ]
    );
}
