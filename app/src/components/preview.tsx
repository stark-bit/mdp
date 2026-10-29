"use client";

import type { MarkdownIt } from "markdown-it";
import { useCallback, useEffect, useRef, useState } from "react";

import { connect } from "@/lib/connection";
import { replaceKeepingDetails } from "@/lib/details";
import { renderDiagrams, themeColors, type Theme } from "@/lib/diagrams";
import { exportHtml, saveHtml } from "@/lib/export-html";
import { createRenderer } from "@/lib/markdown";
import type { UmlColors } from "@/lib/markdown/plantuml";
import type { PreviewData, PreviewOptions, ServerMessage } from "@/lib/protocol";

import { DownloadButton } from "./download-button";
import { ThemeToggle } from "./theme-toggle";

interface Header {
  name: string;
  pageTitle: string;
  showFilename: boolean;
  editable: boolean;
}

// how long to wait for more keystrokes before re-rendering
const RENDER_DEBOUNCE_MS = 16;

// the url path of a preview page: /files/<path>, a raw absolute path whose
// last segment has a markdown extension, or null
export function pageFromUrl(): string | null {
  const path = window.location.pathname;
  if (/^\/files\/.+$/.test(path)) return path;
  const last = path.split("/").pop() ?? "";
  return /\.(md|markdown|mkd|mdown|mkdn|mdwn)$/i.test(last) ? path : null;
}

// file name without directory and extension
export function displayName(path: string): string {
  const base = path.split(/[\\/]/).pop() ?? "";
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(0, dot) : base;
}

function systemTheme(): Theme {
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

function applyTheme(theme: Theme) {
  document.documentElement.classList.toggle("dark", theme === "dark");
  document.documentElement.dataset.theme = theme;
}

function umlColors(): UmlColors | undefined {
  const c = themeColors([
    "foreground",
    "muted-foreground",
    "secondary",
    "muted",
  ] as const);
  if (!c.foreground || !c["muted-foreground"] || !c.secondary || !c.muted)
    return undefined;
  return {
    text: c.foreground,
    line: c["muted-foreground"],
    node: c.secondary,
    muted: c.muted,
  };
}

export function Preview() {
  const bodyRef = useRef<HTMLElement>(null);
  const [page, setPage] = useState<string | null>();
  const [connected, setConnected] = useState(true);
  const [header, setHeader] = useState<Header>();
  // server theme until the user picks one with the toggle
  const [editorTheme, setEditorTheme] = useState<Theme>();
  const [chosenTheme, setChosenTheme] = useState<Theme>();
  const theme = chosenTheme ?? editorTheme;

  const md = useRef<MarkdownIt>(undefined);
  const options = useRef<PreviewOptions>({});
  const source = useRef<string>(undefined);
  const renderTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  // read by the socket callback, which must not change on every toggle
  const chosenThemeRef = useRef(chosenTheme);
  useEffect(() => {
    chosenThemeRef.current = chosenTheme;
  }, [chosenTheme]);

  useEffect(() => setPage(pageFromUrl()), []);

  const render = useCallback(async (theme: Theme) => {
    const body = bodyRef.current;
    if (!body || !md.current || source.current === undefined) return;
    // diagrams read the theme's colors, so it must be in place before them
    applyTheme(theme);
    replaceKeepingDetails(
      body,
      md.current.render(source.current, { umlColors: umlColors() }),
    );
    await renderDiagrams(body, options.current, theme);
  }, []);

  const onRefresh = useCallback(
    (data: PreviewData) => {
      options.current = data.options ?? {};
      md.current ??= createRenderer(options.current);

      const nextEditorTheme =
        data.theme === "dark" || data.theme === "light"
          ? data.theme
          : systemTheme();
      setEditorTheme(nextEditorTheme);
      setHeader({
        name: displayName(data.name),
        pageTitle: data.pageTitle ?? "",
        showFilename: !data.options?.disable_filename,
        editable: Boolean(data.options?.content_editable),
      });

      const text = data.content.join("\n");
      if (text === source.current) return;
      const firstRender = source.current === undefined;
      source.current = text;
      clearTimeout(renderTimer.current);
      const run = () => render(chosenThemeRef.current ?? nextEditorTheme);
      if (firstRender) {
        run();
      } else {
        renderTimer.current = setTimeout(run, RENDER_DEBOUNCE_MS);
      }
    },
    [render],
  );

  const onMessage = useCallback(
    (message: ServerMessage) => {
      if (message.type === "refresh_content") onRefresh(message.data);
    },
    [onRefresh],
  );

  useEffect(() => {
    if (!page) return;
    return connect(page, onMessage, setConnected);
  }, [page, onMessage]);

  useEffect(() => {
    if (theme) applyTheme(theme);
  }, [theme]);

  useEffect(() => {
    if (header)
      document.title = header.pageTitle.replace("${name}", header.name);
  }, [header]);

  const toggleTheme = () => {
    const next = theme === "dark" ? "light" : "dark";
    setChosenTheme(next);
    // diagrams are drawn with theme colors
    render(next);
  };

  const download = async () => {
    const body = bodyRef.current;
    if (!body || !header) return;
    const title = header.pageTitle.replace("${name}", header.name) || header.name;
    const html = await exportHtml(body, {
      title,
      theme: theme ?? "light",
      lang: document.documentElement.lang || "en",
    });
    saveHtml(`${header.name}.html`, html);
  };

  if (page === null) {
    return (
      <p className="notice">
        Open this page by running the preview server on a markdown file.
      </p>
    );
  }

  return (
    <main>
      <div
        id="page-ctn"
        contentEditable={header?.editable}
        suppressContentEditableWarning
      >
        {header?.showFilename && (
          <header id="page-header">
            <h3>
              <svg
                viewBox="0 0 16 16"
                width="16"
                height="16"
                aria-hidden="true"
              >
                <path
                  fillRule="evenodd"
                  d="M3 5h4v1H3V5zm0 3h4V7H3v1zm0 2h4V9H3v1zm11-5h-4v1h4V5zm0 2h-4v1h4V7zm0 2h-4v1h4V9zm2-6v9c0 .55-.45 1-1 1H9.5l-1 1-1-1H2c-.55 0-1-.45-1-1V3c0-.55.45-1 1-1h5.5l1 1 1-1H15c.55 0 1 .45 1 1zm-8 .5L7.5 3H2v9h6V3.5zm7-.5H9.5l-.5.5V12h6V3z"
                />
              </svg>
              {header.name}
            </h3>
            <div className="header-actions">
              {!connected && (
                <span className="status">Disconnected</span>
              )}
              <DownloadButton onDownload={download} />
              <ThemeToggle theme={theme ?? "light"} onToggle={toggleTheme} />
            </div>
          </header>
        )}
        <section className="markdown-body" ref={bodyRef} />
      </div>
    </main>
  );
}
