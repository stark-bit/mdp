use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mkdp-config-test-{}-{}-{nanos}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn load(args: &[&str]) -> Config {
    Config::load_and_parse(args.iter().map(|s| s.to_string()))
}

#[test]
fn defaults_without_a_config_file() {
    let config = load(&[]);
    assert!(!config.open_to_the_world);
    assert_eq!(config.port, None);
    assert_eq!(config.browser, Value::Null);
    assert_eq!(config.open_ip, None);
    assert!(!config.combine_preview);
    assert_eq!(config.images_path, None);
    assert_eq!(config.markdown_css, None);
    assert_eq!(config.highlight_css, None);
    assert_eq!(config.theme_css, None);
    assert!(!config.no_open);
    assert!(config.files.is_empty());
    assert_eq!(config.preview_options, Value::Null);
}

#[test]
fn parses_a_config_file() {
    let tmp = TempDir::new();
    let file = tmp.file(
        "config.toml",
        concat!(
            "open_to_the_world = true\n",
            "port = 8080\n",
            "browser = \"firefox\"\n",
            "open_ip = \"192.0.2.7\"\n",
            "combine_preview = true\n",
            "images_path = \"assets\"\n",
            "markdown_css = \"markdown.css\"\n",
            "highlight_css = \"highlight.css\"\n",
            "theme_css = \"theme/globals.css\"\n",
            "theme = \"dark\"\n",
            "page_title = \"${name}\"\n",
            "unknown_key = \"ignored\"\n",
            "[preview_options]\n",
            "disable_filename = 1\n",
        ),
    );
    let config = load(&["--config", &file.to_string_lossy()]);
    assert!(config.open_to_the_world);
    assert_eq!(config.port, Some(8080));
    assert_eq!(config.browser, json!("firefox"));
    assert_eq!(config.open_ip.as_deref(), Some("192.0.2.7"));
    assert!(config.combine_preview);
    assert_eq!(config.images_path.as_deref(), Some(Path::new("assets")));
    assert_eq!(
        config.markdown_css.as_deref(),
        Some(Path::new("markdown.css"))
    );
    assert_eq!(
        config.highlight_css.as_deref(),
        Some(Path::new("highlight.css"))
    );
    assert_eq!(
        config.theme_css.as_deref(),
        Some(Path::new("theme/globals.css"))
    );
    assert_eq!(config.theme, "dark");
    assert_eq!(config.page_title, "${name}");
    assert_eq!(config.preview_options["disable_filename"], 1);
}

#[test]
fn flags_override_the_config_file() {
    let tmp = TempDir::new();
    let file = tmp.file(
        "config.toml",
        "port = 8080\nbrowser = \"firefox\"\nopen_to_the_world = true\n",
    );
    let config = load(&[
        "--config",
        &file.to_string_lossy(),
        "--port",
        "9000",
        "--no-open",
    ]);
    assert_eq!(config.port, Some(9000));
    assert_eq!(config.browser, json!("firefox"));
    assert!(config.open_to_the_world);
    assert!(config.no_open);
}

#[test]
fn positional_args_become_files() {
    let config = load(&["a.md", "docs/b.md", "--no-open", "c.md"]);
    assert_eq!(
        config.files,
        vec![
            PathBuf::from("a.md"),
            PathBuf::from("docs/b.md"),
            PathBuf::from("c.md")
        ]
    );
}

#[test]
fn browser_takes_a_name_a_list_or_json() {
    assert_eq!(load(&["--browser", "firefox"]).browser, json!("firefox"));
    assert_eq!(
        load(&["--browser", "firefox,-P,work"]).browser,
        json!(["firefox", "-P", "work"])
    );
    assert_eq!(
        load(&["--browser", r#"["firefox", "-P", "work"]"#]).browser,
        json!(["firefox", "-P", "work"])
    );
    assert_eq!(load(&["--browser"]).browser, Value::Null);
}

#[test]
fn port_flag_follows_the_old_semantics() {
    assert_eq!(load(&["--port", "8080"]).port, Some(8080));
    assert_eq!(load(&["--port", " 9000 "]).port, Some(9000));
    assert_eq!(load(&["--port", "65535"]).port, Some(65535));
    for value in ["0", "", "80 80", "port", "65536", "-1", "80.5"] {
        assert_eq!(load(&["--port", value]).port, None, "{value}");
    }
    assert_eq!(load(&["--port"]).port, None);
}

#[test]
fn port_parsing_accepts_numbers_and_strings() {
    assert_eq!(parse_port(&json!(8080)), Some(8080));
    assert_eq!(parse_port(&json!("8080")), Some(8080));
    assert_eq!(parse_port(&json!(" 9000 ")), Some(9000));
    assert_eq!(parse_port(&json!(65535)), Some(65535));
    for value in [
        json!(0),
        json!("0"),
        json!(""),
        json!(65536),
        json!(-1),
        json!("80 80"),
        json!("port"),
        json!(true),
        json!(null),
        json!(80.5),
    ] {
        assert_eq!(parse_port(&value), None, "{value}");
    }
}

#[test]
fn truthy_follows_vim_semantics() {
    for value in [
        json!(1),
        json!(-1),
        json!(0.5),
        json!("1"),
        json!("yes"),
        json!(true),
    ] {
        assert!(truthy(&value), "{value}");
    }
    for value in [
        json!(0),
        json!(0.0),
        json!(""),
        json!("0"),
        json!(false),
        json!(null),
        json!([1]),
    ] {
        assert!(!truthy(&value), "{value}");
    }
}

#[test]
fn var_string_moves_strings_verbatim() {
    assert_eq!(var_string(json!("Open Me")), "Open Me");
    assert_eq!(var_string(json!(null)), "");
    assert_eq!(var_string(json!(12)), "12");
}
