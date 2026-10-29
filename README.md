<h1 align="center"> ✨ Markdown Preview ✨ </h1>

Preview markdown files in your browser with live updates, math, diagrams and
flexible configuration — from a single command, no editor required.

```sh
mm README.md
```

![animation of Markdown Preview with its own README.md](https://user-images.githubusercontent.com/5492542/47603494-28e90000-da1f-11e8-9079-30646e551e7a.gif)

## Features

- Live updates as you edit the file, no manual reloading.
- Download the rendered preview as a standalone HTML file ([details](https://mkdp.sammaji.com/features/live-preview#download-as-html)).
- One self-contained binary, everything embedded: no Node.js at runtime.
- [Math](https://mkdp.sammaji.com/features/markdown#math) with KaTeX, including chemistry via
  mhchem.
- [Diagrams](https://mkdp.sammaji.com/features/diagrams): Mermaid (with a full-screen viewer and
  the ELK layout), PlantUML, Graphviz, flowchart.js, js-sequence-diagrams and
  Chart.js.
- [GitHub-flavoured extras](https://mkdp.sammaji.com/features/markdown): alerts, tables of
  contents, task lists, footnotes, emoji, front matter, local images with
  sizes.
- [Themes](https://mkdp.sammaji.com/features/themes): any
  [shadcn/ui](https://ui.shadcn.com/themes) or [tweakcn](https://tweakcn.com)
  theme with your own fonts, or your own CSS.
- [Share the preview](https://mkdp.sammaji.com/features/browser) with a phone or another
  machine, safely.

## Installation

### Download a release binary

Pre-built binaries are on the
[releases page](https://github.com/sammaji/markdown-preview.nvim/releases) for:

| System  | Architectures |
| ------- | ------------- |
| macOS   | x64, arm64    |
| Linux   | x64, arm64    |
| FreeBSD | x64           |
| Windows | x64           |

Download the archive for your system (for example `markdown-preview-macos-arm64.tar.gz`),
extract it and put the `md` binary on your `PATH`:

```sh
tar -xzf markdown-preview-macos-arm64.tar.gz
sudo mv markdown-preview-macos-arm64 /usr/local/bin/mm
```

On Windows, extract `markdown-preview-win.zip` and add the folder to `PATH`.

### Building from source

```sh
git clone https://github.com/sammaji/markdown-preview.nvim.git
cd markdown-preview.nvim
cargo build --release
```

The binary is at `target/release/mm`. This needs
[Rust](https://rustup.rs) 1.86+ and Node.js 20.9+ with pnpm or npx (Node.js is
only needed to build the preview page, which is embedded into the binary — it
is not needed to run it).

## Quickstart

```sh
mm README.md
```

The server prints the preview URL and opens it in your browser:

```text
http://localhost:8123/files/README.md
```

Edit `README.md` in any editor and the page updates by itself. Press `Ctrl-C`
to stop the server. Use `--no-open` to print the URL without opening a
browser.

To see every feature, preview [`examples/features.md`](examples/features.md).

## Command-line usage

```text
mm [flags] [files...]
```

| Flag                | Description                                                                 |
| ------------------- | --------------------------------------------------------------------------- |
| `[files...]`        | Markdown files to preview; each gets its own page and URL                   |
| `--config <path>`   | Config file to read; default `markdown-preview.toml` in the current directory |
| `--port <n>`        | Port to serve on; the next free port is used when the first 20 are taken    |
| `--root <dir>`      | Directory the previewed files must live under; default the current directory |
| `--browser <cmd...>`| Browser to open the URL in; a name, or a command with its arguments          |
| `--no-open`         | Print the URL but do not open a browser                                     |
| `--version`         | Print the version                                                           |

## Configuration

Options live in a `markdown-preview.toml` next to where you run the server, or
in a file named with `--config`:

```toml
# "dark" or "light"; by default the page follows the system
theme = "dark"
# a browser name, or a command with arguments; the URL is appended
browser = ["firefox", "--new-window"]
# listen on all interfaces, so other devices can open the preview
open_to_the_world = true
# directory relative image paths are resolved against
images_path = "assets"

[preview_options]
# YAML front matter: "hide", "panel" or "raw"
front_matter = "panel"
```

Command-line flags override the config file. See the
[configuration reference](https://mkdp.sammaji.com/configuration) for every
option, and [Theming](https://mkdp.sammaji.com/features/themes) for styling the
page.

## Documentation

The documentation is at [mkdp.sammaji.com](https://mkdp.sammaji.com).

- [Quickstart](https://mkdp.sammaji.com/quickstart).
- [Installation](https://mkdp.sammaji.com/installation).
- [Live preview](https://mkdp.sammaji.com/features/live-preview),
  [Markdown](https://mkdp.sammaji.com/features/markdown), [Diagrams](https://mkdp.sammaji.com/features/diagrams),
  [Theming](https://mkdp.sammaji.com/features/themes),
  [Browser and sharing](https://mkdp.sammaji.com/features/browser).
- [Configuration reference](https://mkdp.sammaji.com/configuration).
- [FAQ](https://mkdp.sammaji.com/faq).
- [Contributing](https://mkdp.sammaji.com/contributing).

## Buy me a coffee

[Buy me a coffee](https://buymeacoffee.com/sammaji15)

## Star History

<a href="https://www.star-history.com/?repos=sammaji%2Fmarkdown-preview.nvim&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=sammaji/markdown-preview.nvim&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=sammaji/markdown-preview.nvim&type=date&theme=light&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=sammaji/markdown-preview.nvim&type=date&legend=top-left" />
 </picture>
</a>
