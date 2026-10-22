use std::collections::HashMap;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use include_dir::{include_dir, Dir};
use notify::Watcher;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::{error, info, opener};

const LOG: &str = "server";

/// The preview page: static export of the Next.js app in `app/`, built by
/// build.rs.
static OUT: Dir = include_dir!("$CARGO_MANIFEST_DIR/app/out");

/// How many ports after the preferred one to try before letting the OS pick.
const PORT_ATTEMPTS: u16 = 20;

/// Characters escaped in the file names of /files/ urls.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// What a preview page shows: a markdown file under the root, read from
/// disk.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Target {
    File(PathBuf),
}

/// A preview page connected over the WebSocket.
struct Client {
    id: u64,
    tx: mpsc::UnboundedSender<Message>,
}

pub struct App {
    config: Config,
    /// Connected preview pages, keyed by what they show.
    clients: Mutex<HashMap<Target, Vec<Client>>>,
    next_client_id: AtomicU64,
    /// Watches the parent directories of the previewed files; `None` when
    /// file watching is unavailable.
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    /// Parent directories already watched.
    watched: Mutex<HashSet<PathBuf>>,
    open_to_the_world: bool,
    port: u16,
    token: String,
    /// The served root, canonical. Files under it are previewed at
    /// /files/<path relative to it>.
    root: Option<PathBuf>,
}

pub async fn run(config: Config) {
    let listener = match bind(config.open_to_the_world, config.port).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("[markdown-preview]: failed to start server: {err}");
            std::process::exit(1);
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
    info!(LOG, "server run: {port}");

    let root = match &config.root {
        Some(root) => root.canonicalize().ok(),
        None => std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.canonicalize().ok()),
    };
    let app = App::new(config, port, root);
    let router = router(app.clone());
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router).await {
            error!(LOG, "http server error: {err}");
        }
    });

    app.spawn_watcher();
    app.open_files().await;
    if tokio::signal::ctrl_c().await.is_ok() {
        info!(LOG, "shutting down");
    }
}

/// Binds the preferred port, falling back to the following ports and finally
/// to one picked by the OS. A port given with `--port` must be served or the
/// server fails: a user who picked one expects that one, nearby.
async fn bind(open_to_the_world: bool, preferred: Option<u16>) -> std::io::Result<TcpListener> {
    let host = if open_to_the_world {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    };
    let start = preferred.unwrap_or_else(|| {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        8080 + (millis % 1000) as u16
    });
    let mut last_error = None;
    for port in (0..PORT_ATTEMPTS).filter_map(|i| start.checked_add(i)) {
        if ipv6_taken(port) {
            info!(LOG, "port {port} unavailable: in use on IPv6");
            continue;
        }
        match TcpListener::bind(SocketAddr::new(host, port)).await {
            Ok(listener) => return Ok(listener),
            Err(err) => {
                info!(LOG, "port {port} unavailable: {err}");
                last_error = Some(err);
            }
        }
    }
    match preferred {
        Some(_) => Err(last_error.unwrap_or_else(|| {
            std::io::Error::other(format!(
                "ports {start}..{} are taken",
                start + PORT_ATTEMPTS
            ))
        })),
        None => TcpListener::bind(SocketAddr::new(host, 0)).await,
    }
}

fn ipv6_taken(port: u16) -> bool {
    let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port);
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/ws", get(websocket))
        .fallback(route)
        .with_state(app)
}

impl App {
    fn new(config: Config, port: u16, root: Option<PathBuf>) -> Arc<App> {
        let open_to_the_world = config.open_to_the_world;
        Arc::new(App {
            config,
            clients: Mutex::new(HashMap::new()),
            next_client_id: AtomicU64::new(1),
            watcher: Mutex::new(None),
            watched: Mutex::new(HashSet::new()),
            open_to_the_world,
            port,
            token: new_token(),
            root,
        })
    }

    fn cookie_name(&self) -> String {
        format!("mkdp_token_{}", self.port)
    }

    fn authorized(&self, uri: &Uri, headers: &HeaderMap) -> bool {
        !self.open_to_the_world
            || query_param(uri, "token") == Some(self.token.as_str())
            || cookies(headers)
                .any(|(name, value)| name == self.cookie_name() && value == self.token)
    }

    /// Serves one preview page until it disconnects.
    async fn serve_client(self: Arc<Self>, target: Target, mut socket: WebSocket) {
        let id = self.next_client_id.fetch_add(1, Ordering::Relaxed);
        info!(LOG, "client connect: {id} {target:?}");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let data = match &target {
            Target::File(path) => self.file_data(path).await,
        };
        if let Some(data) = data {
            let _ = tx.send(message(json!({ "type": "refresh_content", "data": data })));
        }
        // edits to the file's directory are what refresh this page
        match &target {
            Target::File(path) => self.watch_files(std::slice::from_ref(path)),
        }
        self.clients
            .lock()
            .unwrap()
            .entry(target)
            .or_default()
            .push(Client { id, tx });

        // the map holds the only sender, so removing the client ends the loop
        // once its queued messages are sent
        loop {
            tokio::select! {
                outgoing = rx.recv() => match outgoing {
                    Some(msg) => if socket.send(msg).await.is_err() { break },
                    None => break,
                },
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    // pings are answered by axum; pages send nothing else
                    Some(Ok(_)) => {}
                },
            }
        }

        info!(LOG, "disconnect: {id}");
        let _ = socket.send(Message::Close(None)).await;
        drop(socket);
        for clients in self.clients.lock().unwrap().values_mut() {
            clients.retain(|c| c.id != id);
        }
    }

    /// Reads a markdown file from disk in the shape the page consumes.
    async fn file_data(&self, path: &Path) -> Option<Value> {
        let text = match tokio::fs::read_to_string(path).await {
            Ok(text) => text,
            Err(err) => {
                error!(LOG, "failed to read {}: {err}", path.display());
                return None;
            }
        };
        let content: Vec<Value> = text.lines().map(Value::from).collect();
        Some(json!({
            "options": self.config.preview_options,
            "isActive": 0,
            "winline": 0,
            "winheight": 0,
            "cursor": [0, 0, 0, 0],
            "pageTitle": self.config.page_title,
            "theme": self.config.theme,
            "name": path.to_string_lossy(),
            "content": content,
        }))
    }

    /// Sends `msg` to the pages showing `target`, or to all pages.
    fn broadcast(&self, target: Option<&Target>, msg: Value) -> usize {
        let msg = message(msg);
        let clients = self.clients.lock().unwrap();
        let targets: Vec<&Client> = match target {
            Some(target) => clients.get(target).into_iter().flatten().collect(),
            None => clients.values().flatten().collect(),
        };
        targets
            .iter()
            .filter(|c| c.tx.send(msg.clone()).is_ok())
            .count()
    }

    fn has_clients(&self, target: &Target) -> bool {
        self.clients
            .lock()
            .unwrap()
            .get(target)
            .is_some_and(|c| !c.is_empty())
    }

    async fn refresh_file(&self, path: &Path) {
        let target = Target::File(path.to_path_buf());
        if !self.has_clients(&target) {
            return;
        }
        info!(LOG, "refresh page: {}", path.display());
        if let Some(data) = self.file_data(path).await {
            self.broadcast(
                Some(&target),
                json!({ "type": "refresh_content", "data": data }),
            );
        }
    }

    /// Watches the parents of the configured files, and the parents of every
    /// file a page later opens, and refreshes pages when a file changes.
    fn spawn_watcher(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        // the callback runs on notify's thread; file reads and broadcasts
        // need the async runtime
        let runtime = tokio::runtime::Handle::current();
        let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            let Some(app) = weak.upgrade() else { return };
            match result {
                Ok(event) => app.on_fs_event(event, &runtime),
                Err(err) => error!(LOG, "file watch failed: {err}"),
            }
        });
        match watcher {
            Ok(watcher) => {
                *self.watcher.lock().unwrap() = Some(watcher);
                self.watch_files(&self.config.files);
            }
            Err(err) => error!(LOG, "file watching unavailable: {err}"),
        }
    }

    /// Watches the parent directory of each file, once.
    fn watch_files(&self, files: &[PathBuf]) {
        let mut watcher = self.watcher.lock().unwrap();
        let Some(watcher) = watcher.as_mut() else {
            return;
        };
        let mut watched = self.watched.lock().unwrap();
        for file in files {
            let Some(parent) = file
                .canonicalize()
                .ok()
                .and_then(|f| f.parent().map(Path::to_path_buf))
            else {
                continue;
            };
            if watched.insert(parent.clone()) {
                if let Err(err) = watcher.watch(&parent, notify::RecursiveMode::NonRecursive) {
                    error!(LOG, "cannot watch {}: {err}", parent.display());
                    watched.remove(&parent);
                }
            }
        }
    }

    /// Refreshes the pages of changed markdown files.
    fn on_fs_event(self: &Arc<Self>, event: notify::Event, runtime: &tokio::runtime::Handle) {
        match event.kind {
            notify::EventKind::Modify(_)
            | notify::EventKind::Create(_)
            | notify::EventKind::Remove(_) => {}
            // reads and metadata-only events are not edits
            _ => return,
        }
        for path in event.paths {
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if !is_markdown(&path) {
                continue;
            }
            let app = Arc::clone(self);
            runtime.spawn(async move { app.refresh_file(&path).await });
        }
    }

    /// Prints each configured file's url and opens it in a browser, unless
    /// `--no-open` was given.
    async fn open_files(&self) {
        for file in &self.config.files {
            let path = file.canonicalize().unwrap_or_else(|_| file.clone());
            let url = self.page_url(&self.files_path(&path));
            println!("{url}");
            if self.config.no_open {
                continue;
            }
            let browser = opener::Browser::from_var(&self.config.browser);
            info!(LOG, "open page [{browser:?}]: {url}");
            if let Err(msg) = opener::open(&url, &browser) {
                error!(LOG, "{msg}");
            }
        }
    }

    /// The url a browser is opened at: the host to use, plus the token when
    /// the server listens to the world.
    fn page_url(&self, page: &str) -> String {
        let open_ip = self.config.open_ip.clone().unwrap_or_default();
        let host = if !open_ip.is_empty() {
            open_ip
        } else if self.open_to_the_world {
            local_ip().unwrap_or_else(|| "localhost".into())
        } else {
            "localhost".into()
        };
        let mut url = format!("http://{host}:{}{page}", self.port);
        if self.open_to_the_world {
            url.push_str(&format!("?token={}", self.token));
        }
        url
    }
}

impl App {
    /// The url path of the page for `path`: /files/<path relative to the
    /// root>, or the file's canonical path when it is not under the root.
    fn files_path(&self, path: &Path) -> String {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let relative = self
            .root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok());
        match relative {
            Some(relative) if !relative.as_os_str().is_empty() => {
                let segments: Vec<String> = relative
                    .components()
                    .map(|c| {
                        utf8_percent_encode(&c.as_os_str().to_string_lossy(), PATH_SEGMENT)
                            .to_string()
                    })
                    .collect();
                if !segments.is_empty() {
                    return format!("/files/{}", segments.join("/"));
                }
            }
            _ => {}
        }
        path.to_string_lossy().into_owned()
    }

    /// The file a /files/ url path names: under the root, after following
    /// symlinks, and existing.
    fn resolve_file(&self, encoded: &str) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let decoded = percent_decode_str(encoded).decode_utf8().ok()?;
        let relative = Path::new(decoded.as_ref());
        // no `..`, no absolute paths and, on Windows, no drive letters
        if decoded.is_empty()
            || decoded.contains('\0')
            || !relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        {
            return None;
        }
        let path = root.join(relative).canonicalize().ok()?;
        (path.starts_with(root) && path.is_file()).then_some(path)
    }

    /// What the page at url path `page` shows.
    async fn target(&self, page: &str) -> Option<Target> {
        let path = self.resolve_file(page.strip_prefix("/files/")?)?;
        is_markdown(&path).then(|| Target::File(path))
    }
}

fn message(msg: Value) -> Message {
    Message::Text(msg.to_string().into())
}

async fn websocket(
    State(app): State<Arc<App>>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !app.authorized(&uri, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // the url path of the page, e.g. /page/3 or /files/docs/a.md
    let page = query_param(&uri, "path").map(|p| percent_decode_str(p).decode_utf8_lossy());
    let target = match page {
        Some(page) => app.target(&page).await,
        None => None,
    };
    match target {
        Some(target) => ws.on_upgrade(move |socket| app.serve_client(target, socket)),
        None => StatusCode::BAD_REQUEST.into_response(),
    }
}

async fn route(State(app): State<Arc<App>>, req: Request) -> Response {
    let path = req.uri().path();
    let files = path.starts_with("/files/");
    let page = path == "/";
    let private = page || files || path.starts_with("/_assets/") || path.starts_with("/_theme/");
    if private && !app.authorized(req.uri(), req.headers()) {
        return unauthorized();
    }
    if files && app.target(path).await.is_none() {
        // a link from a page to an image or video next to it
        return match app.resolve_file(&path[7..]) {
            Some(file) if is_media(&file) => match tokio::fs::read(&file).await {
                Ok(bytes) => file_response(&file.to_string_lossy(), bytes),
                Err(_) => not_found(),
            },
            _ => not_found(),
        };
    }
    if page || files {
        let mut response = embedded("index.html");
        if app.open_to_the_world {
            let cookie = format!(
                "{}={}; Path=/; HttpOnly; SameSite=Strict",
                app.cookie_name(),
                app.token
            );
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().insert(header::SET_COOKIE, value);
            }
        }
        return response;
    }
    if let Some(file) = path.strip_prefix("/_theme/") {
        return theme_file(&app, file).await;
    }
    let custom_css = match path {
        "/_static/markdown.css" => app.config.markdown_css.clone(),
        "/_static/highlight.css" => app.config.highlight_css.clone(),
        _ => None,
    };
    if let Some(css) = custom_css {
        let css = css.to_string_lossy().into_owned();
        if !css.is_empty() {
            match tokio::fs::read(&css).await {
                Ok(bytes) => return file_response(&css, bytes),
                Err(err) => error!(LOG, "load diy css fail: {path} {css}: {err}"),
            }
        }
    }
    if let Some(image) = path.strip_prefix("/_assets/") {
        // the url path of the page showing the image
        let page = req
            .headers()
            .get(header::REFERER)
            .and_then(|r| r.to_str().ok())
            .map(|r| r.split(['?', '#']).next().unwrap_or(r))
            .and_then(|r| r.split_once("://"))
            .and_then(|(_, rest)| rest.find('/').map(|slash| &rest[slash..]));
        let target = match page {
            Some(page) => app.target(page).await,
            None => None,
        };
        return local_image(&app, target, image)
            .await
            .unwrap_or_else(not_found);
    }
    embedded(path.trim_start_matches('/'))
}

async fn local_image(app: &App, target: Option<Target>, image: &str) -> Option<Response> {
    info!(LOG, "image route: {image}");
    let target = target?;
    let mut file_dir = app
        .config
        .images_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if file_dir.is_empty() {
        file_dir = match target {
            Target::File(path) => path.parent()?.to_string_lossy().into_owned(),
        };
    }
    if std::env::var_os("MINGW_HOME").is_some() && !file_dir.contains(':') {
        // unix-like path such as /Z/x/y from a MinGW vim; convert to Z:\x\y
        if let Ok(out) = std::process::Command::new("cygpath.exe")
            .args(["-w", "-a", &file_dir])
            .output()
        {
            file_dir = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
        }
    }

    // the page encodes the src once and the browser may encode it again
    let decoded = percent_decode_str(image).decode_utf8_lossy().into_owned();
    let decoded = percent_decode_str(&decoded)
        .decode_utf8_lossy()
        .replace("\\ ", " ");
    let file_dir = PathBuf::from(file_dir);
    let rooted = decoded.starts_with(['/', '\\']) || Path::new(&decoded).is_absolute();
    let mut img_path = if rooted {
        PathBuf::from(&decoded)
    } else {
        file_dir.join(&decoded)
    };
    if rooted && !img_path.exists() {
        // "/images/x.png" may be relative to a parent directory of the file
        let relative = decoded.trim_start_matches(['/', '\\']);
        if let Some(found) = file_dir
            .ancestors()
            .map(|dir| dir.join(relative))
            .find(|p| p.exists())
        {
            img_path = found;
        }
    }
    info!(LOG, "imgPath {}", img_path.display());

    // the route takes any path, so it must not become a way to read the
    // user's files
    if !is_media(&img_path) {
        error!(LOG, "not an image: {}", img_path.display());
        return None;
    }
    if img_path.is_file() {
        if let Ok(bytes) = tokio::fs::read(&img_path).await {
            return Some(file_response(&img_path.to_string_lossy(), bytes));
        }
    }
    error!(LOG, "image not exists: {}", img_path.display());
    None
}

async fn theme_file(app: &App, file: &str) -> Response {
    let theme = app
        .config
        .theme_css
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if file != "theme.css" {
        return theme_font(&theme, file).await.unwrap_or_else(not_found);
    }
    let mut css = String::new();
    if !theme.is_empty() {
        match tokio::fs::read_to_string(&theme).await {
            Ok(text) => css = theme_css(&text),
            Err(err) => error!(LOG, "load theme css fail: {theme}: {err}"),
        }
    }
    file_response("theme.css", css)
}

async fn theme_font(theme: &str, file: &str) -> Option<Response> {
    let decoded = percent_decode_str(file).decode_utf8_lossy();
    if theme.is_empty() || !is_font(Path::new(&*decoded)) {
        return None;
    }
    let dir = Path::new(theme).parent()?.canonicalize().ok()?;
    let font = dir.join(&*decoded).canonicalize().ok()?;
    if !font.starts_with(&dir) {
        return None;
    }
    let bytes = tokio::fs::read(&font).await.ok()?;
    Some(file_response(&font.to_string_lossy(), bytes))
}

fn is_font(path: &Path) -> bool {
    mime_guess::from_path(path)
        .iter()
        .any(|mime| mime.type_() == mime_guess::mime::FONT)
}

pub(crate) fn theme_css(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find("--") {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let name_len = from[2..]
            .find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
            .map_or(from.len(), |n| n + 2);
        let (name, after) = from.split_at(name_len);
        out.push_str(name);
        rest = after;
        let Some(value) = after.strip_prefix(':') else {
            continue;
        };
        let end = value.find([';', '}']).unwrap_or(value.len());
        let (value, tail) = value.split_at(end);
        if is_hsl_channels(value.trim()) {
            out.push_str(&format!(": hsl({})", value.trim()));
        } else {
            out.push(':');
            out.push_str(value);
        }
        rest = tail;
    }
    out.push_str(rest);
    out.lines()
        .filter(|line| !is_package_import(line.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `222.2 47.4% 11.2%`, optionally with ` / 50%` alpha
fn is_hsl_channels(value: &str) -> bool {
    let (channels, alpha) = match value.split_once('/') {
        Some((channels, alpha)) => (channels, Some(alpha.trim())),
        None => (value, None),
    };
    let number = |s: &str| !s.is_empty() && s.parse::<f64>().is_ok();
    let percent = |s: &str| s.strip_suffix('%').is_some_and(number);
    let parts: Vec<&str> = channels.split_whitespace().collect();
    parts.len() == 3
        && number(parts[0].trim_end_matches("deg"))
        && percent(parts[1])
        && percent(parts[2])
        && alpha.is_none_or(|a| number(a) || percent(a))
}

/// `@import "tailwindcss";`, but not `@import url(https://fonts...)` or a file
fn is_package_import(line: &str) -> bool {
    let Some(target) = line.strip_prefix("@import") else {
        return false;
    };
    let target = target.trim().trim_end_matches(';').trim();
    let Some(name) = target
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .or_else(|| target.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')))
    else {
        return false;
    };
    !(name.contains(':') || name.starts_with(['.', '/']) || name.ends_with(".css"))
}

/// Files a /files/ url previews when no buffer has them open.
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["md", "markdown", "mkd", "mdown", "mkdn", "mdwn"]
                .contains(&ext.to_ascii_lowercase().as_str())
        })
}

fn is_media(path: &Path) -> bool {
    mime_guess::from_path(path).iter().any(|mime| {
        matches!(
            mime.type_(),
            mime_guess::mime::IMAGE | mime_guess::mime::AUDIO | mime_guess::mime::VIDEO
        )
    })
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        "markdown-preview.nvim: open the preview from the editor, this link is missing its token",
    )
        .into_response()
}

fn query_param<'a>(uri: &'a Uri, name: &str) -> Option<&'a str> {
    uri.query()?.split('&').find_map(|kv| {
        kv.split_once('=')
            .filter(|(key, _)| *key == name)
            .map(|(_, value)| value)
    })
}

fn cookies(headers: &HeaderMap) -> impl Iterator<Item = (&str, &str)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
}

/// A request without an Origin header does not come from a web page.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|o| o.split_once("://"))
        .map(|(_, host)| host);
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    origin_host.is_some() && origin_host == host
}

/// 128 random bits, hex encoded.
fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("no random numbers from the OS");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn embedded(path: &str) -> Response {
    match OUT.get_file(path) {
        Some(file) => file_response(path, file.contents()),
        None => not_found(),
    }
}

fn not_found() -> Response {
    let body = OUT
        .get_file("404.html")
        .map(|f| f.contents())
        .unwrap_or_default();
    let mut response = file_response("404.html", body);
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

fn file_response(path: &str, body: impl Into<Body>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(body.into());
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

/// The address other machines on the network can reach us on. Connecting a
/// UDP socket sends no packets but makes the OS choose the outbound interface.
fn local_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

#[cfg(test)]
#[path = "server_test.rs"]
pub(crate) mod tests;
