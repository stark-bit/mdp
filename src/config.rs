//! The server's options, loaded from `markdown-preview.toml` and CLI flags.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A browser value as the page/opener expects: null (default), a name, or
/// `[program, arg, ...]`. Parsed by `opener::Browser::from_var`.
pub type BrowserValue = Value;

/// Keys a config file may set. Unknown keys warn and are ignored.
const KEYS: &[&str] = &[
    "open_to_the_world",
    "port",
    "browser",
    "open_ip",
    "combine_preview",
    "images_path",
    "markdown_css",
    "highlight_css",
    "theme_css",
    "theme",
    "page_title",
    "preview_options",
];

// root, files, preview_options, theme and page_title are read by later phases
// combine_preview is editor-only and disappears with the page cleanup
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub open_to_the_world: bool, // mkdp_open_to_the_world
    pub port: Option<u16>,       // mkdp_port
    pub browser: BrowserValue,   // mkdp_browser
    pub open_ip: Option<String>, // mkdp_open_ip
    pub combine_preview: bool,   // mkdp_combine_preview (temporary)
    pub images_path: Option<PathBuf>,
    pub markdown_css: Option<PathBuf>,
    pub highlight_css: Option<PathBuf>,
    pub theme_css: Option<PathBuf>,
    pub theme: String,          // mkdp_theme
    pub page_title: String,     // mkdp_page_title
    pub preview_options: Value, // mkdp_preview_options, verbatim for the page
    pub root: Option<PathBuf>,  // --root; None = cwd (resolved in Phase 3)
    pub no_open: bool,          // --no-open
    pub files: Vec<PathBuf>,    // positional args (used from Phase 3)
}

impl Config {
    /// defaults <- config file (markdown-preview.toml or --config path) <- flags
    pub fn load_and_parse(args: impl IntoIterator<Item = String>) -> Config {
        let args: Vec<String> = args.into_iter().collect();
        let (flags, files, config_path) = parse_args(&args);
        let explicit = config_path.is_some();
        let path = config_path.unwrap_or_else(|| PathBuf::from("markdown-preview.toml"));
        let mut config = load_file(&path, explicit);
        if let Some(port) = flags.port {
            config.port = Some(port);
        }
        if let Some(root) = flags.root {
            config.root = Some(root);
        }
        config.no_open |= flags.no_open;
        if let Some(browser) = flags.browser {
            config.browser = browser;
        }
        config.files = files;
        config
    }

    /// Maps a parsed config file (as JSON) onto the fields.
    fn from_json(value: &Value) -> Config {
        let mut config = Config::default();
        if let Some(v) = value.get("open_to_the_world") {
            config.open_to_the_world = truthy(v);
        }
        if let Some(v) = value.get("port") {
            config.port = parse_port(v);
        }
        if let Some(v) = value.get("browser") {
            config.browser = v.clone();
        }
        if let Some(v) = value.get("open_ip") {
            config.open_ip = optional_string(v);
        }
        if let Some(v) = value.get("combine_preview") {
            config.combine_preview = truthy(v);
        }
        if let Some(v) = value.get("images_path") {
            config.images_path = optional_path(v);
        }
        if let Some(v) = value.get("markdown_css") {
            config.markdown_css = optional_path(v);
        }
        if let Some(v) = value.get("highlight_css") {
            config.highlight_css = optional_path(v);
        }
        if let Some(v) = value.get("theme_css") {
            config.theme_css = optional_path(v);
        }
        if let Some(v) = value.get("theme") {
            config.theme = var_string(v.clone());
        }
        if let Some(v) = value.get("page_title") {
            config.page_title = var_string(v.clone());
        }
        if let Some(v) = value.get("preview_options") {
            config.preview_options = v.clone();
        }
        if let Some(object) = value.as_object() {
            for key in object.keys() {
                if !KEYS.contains(&key.as_str()) {
                    warn(&format!("unknown config key: {key}"));
                }
            }
        }
        config
    }
}

struct Flags {
    port: Option<u16>,
    root: Option<PathBuf>,
    no_open: bool,
    browser: Option<Value>,
    config: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> (Flags, Vec<PathBuf>, Option<PathBuf>) {
    let mut flags = Flags {
        port: None,
        root: None,
        no_open: false,
        browser: None,
        config: None,
    };
    let mut files = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i).map(String::as_str) {
        i += 1;
        match arg {
            "--port" => {
                let value = args.get(i).cloned();
                i += 1;
                flags.port = value.as_deref().and_then(|v| parse_port(&Value::from(v)));
                if flags.port.is_none() {
                    warn(&format!(
                        "invalid --port value: {}",
                        value.as_deref().unwrap_or("")
                    ));
                }
            }
            "--root" | "--config" => {
                let value = args.get(i).cloned();
                i += 1;
                match value {
                    Some(path) if arg == "--root" => flags.root = Some(PathBuf::from(path)),
                    Some(path) => flags.config = Some(PathBuf::from(path)),
                    None => warn(&format!("{arg} needs a value")),
                }
            }
            "--no-open" => flags.no_open = true,
            "--browser" => {
                let mut values = Vec::new();
                while i < args.len() && !args[i].starts_with('-') {
                    values.push(args[i].clone());
                    i += 1;
                }
                flags.browser = Some(browser_value(values));
            }
            _ if arg.starts_with('-') => warn(&format!("unknown flag: {arg}")),
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let config = flags.config.take();
    (flags, files, config)
}

/// `--browser` values: a name, one comma/JSON value or the following args.
fn browser_value(values: Vec<String>) -> Value {
    match values.as_slice() {
        [] => Value::Null,
        [one] => match serde_json::from_str::<Value>(one) {
            Ok(Value::Array(items)) => Value::Array(items),
            _ if one.contains(',') => Value::Array(
                one.split(',')
                    .map(|part| Value::String(part.trim().to_string()))
                    .collect(),
            ),
            _ => Value::String(one.clone()),
        },
        many => Value::Array(many.iter().map(|v| Value::String(v.clone())).collect()),
    }
}

/// Parses the config file; a missing default file is fine, a missing
/// `--config` file is not.
fn load_file(path: &Path, explicit: bool) -> Config {
    let result = std::fs::read_to_string(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))
        .and_then(|text| {
            toml::from_str::<toml::Value>(&text)
                .map_err(|err| format!("invalid TOML in {}: {err}", path.display()))
        })
        .and_then(|value| {
            serde_json::to_value(&value).map_err(|err| format!("cannot convert {value}: {err}"))
        });
    match result {
        Ok(json) => Config::from_json(&json),
        Err(msg) => {
            if explicit {
                warn(&msg);
            }
            Config::default()
        }
    }
}

/// vim booleans arrive as numbers, strings or bools depending on the config
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty() && s != "0",
        _ => false,
    }
}

pub(crate) fn var_string(value: Value) -> String {
    match value {
        Value::String(s) => s,
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn parse_port(value: &Value) -> Option<u16> {
    match value {
        Value::Number(n) => n.as_u64().and_then(|n| u16::try_from(n).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|&port| port != 0)
}

fn optional_string(value: &Value) -> Option<String> {
    let string = var_string(value.clone());
    (!string.is_empty()).then_some(string)
}

fn optional_path(value: &Value) -> Option<PathBuf> {
    optional_string(value).map(PathBuf::from)
}

fn warn(msg: &str) {
    eprintln!("[markdown-preview]: {msg}");
}

#[cfg(test)]
#[path = "config_test.rs"]
pub(crate) mod tests;
