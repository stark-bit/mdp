import type { Theme } from "@/lib/diagrams";

const stylesheetPaths = ["/_next/static/", "/_static/", "/_theme/"];

export interface ExportOptions {
  title: string;
  theme: Theme;
  lang: string;
}

export async function exportHtml(
  body: HTMLElement,
  { title, theme, lang }: ExportOptions,
): Promise<string> {
  const content = body.cloneNode(true) as HTMLElement;
  content
    .querySelectorAll(".diagram-open, .diagram-viewer, script")
    .forEach((element) => element.remove());
  const liveCanvases = body.querySelectorAll<HTMLCanvasElement>(".chartjs canvas");
  const clonedCanvases = content.querySelectorAll<HTMLCanvasElement>(".chartjs canvas");
  clonedCanvases.forEach((canvas, index) => {
    const liveCanvas = liveCanvases[index];
    if (liveCanvas) canvasToImage(canvas, liveCanvas);
  });
  await inlineImages(content);
  if (content.hasAttribute("contenteditable")) {
    content.removeAttribute("contenteditable");
  }
  content.querySelectorAll("[contenteditable]").forEach((element) => {
    element.removeAttribute("contenteditable");
  });

  const stylesheets = await Promise.all(
    Array.from(
      document.querySelectorAll<HTMLLinkElement>('link[rel="stylesheet"]'),
    ).map(async (link) => {
      const url = new URL(link.href, document.baseURI);
      const isLocal = url.origin === window.location.origin;
      const isKnownStylesheet = stylesheetPaths.some((path) =>
        url.pathname.startsWith(path),
      );
      if (!isLocal || !isKnownStylesheet || !link.sheet) return "";
      try {
        const css = Array.from(link.sheet.cssRules, (rule) => rule.cssText).join("\n");
        return await inlineCssUrls(css, url.href);
      } catch {
        return "";
      }
    }),
  );
  const styles = stylesheets.map((css) => `<style>\n${css}\n</style>`).join("\n");
  const htmlClass = theme === "dark" ? ' class="dark"' : "";

  return `<!doctype html>\n<html lang="${escapeAttribute(lang)}"${htmlClass} data-theme="${theme}">\n<head>\n<meta charset="utf-8">\n<title>${escapeHtml(title)}</title>\n${styles}\n</head>\n<body>\n<main><div id="page-ctn">${content.outerHTML}</div></main>\n</body>\n</html>`;
}

export function saveHtml(filename: string, html: string): void {
  const blob = new Blob([html], { type: "text/html;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

async function inlineImages(content: HTMLElement): Promise<void> {
  await Promise.all(
    Array.from(content.querySelectorAll<HTMLImageElement>("img")).map(async (image) => {
      if (/^data:/i.test(image.src)) return;
      try {
        const response = await window.fetch(image.src);
        if (!response.ok) return;
        const contentType = response.headers.get("content-type") ?? "application/octet-stream";
        image.src = toDataUri(await response.arrayBuffer(), contentType);
      } catch {
        // Keep images that cannot be fetched.
      }
    }),
  );
}

function canvasToImage(clone: HTMLCanvasElement, live: HTMLCanvasElement): void {
  try {
    const image = document.createElement("img");
    for (const attribute of Array.from(clone.attributes)) {
      image.setAttribute(attribute.name, attribute.value);
    }
    image.src = live.toDataURL("image/png");
    clone.replaceWith(image);
  } catch {
    // Keep canvases that cannot be exported.
  }
}

async function inlineCssUrls(css: string, sheetUrl: string): Promise<string> {
  const pattern = /url\(\s*(?:(['"])(.*?)\1|([^)]*?))\s*\)/gi;
  const matches = Array.from(css.matchAll(pattern));
  const replacements = await Promise.all(
    matches.map(async (match) => {
      const original = match[0];
      const rawUrl = (match[2] ?? match[3] ?? "").trim();
      if (!rawUrl || rawUrl.startsWith("#") || /^data:/i.test(rawUrl)) {
        return original;
      }
      try {
        const assetUrl = new URL(rawUrl, sheetUrl);
        const response = await window.fetch(assetUrl.href);
        if (!response.ok) return original;
        const contentType = response.headers.get("content-type") ?? "application/octet-stream";
        const data = await response.arrayBuffer();
        return `url("${toDataUri(data, contentType)}")`;
      } catch {
        return original;
      }
    }),
  );

  let result = "";
  let cursor = 0;
  matches.forEach((match, index) => {
    const start = match.index ?? 0;
    result += css.slice(cursor, start) + replacements[index];
    cursor = start + match[0].length;
  });
  return result + css.slice(cursor);
}

function toDataUri(data: ArrayBuffer, contentType: string): string {
  const bytes = new Uint8Array(data);
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return `data:${contentType};base64,${btoa(binary)}`;
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => {
    const entities: Record<string, string> = {
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    };
    return entities[char];
  });
}

function escapeAttribute(value: string): string {
  return escapeHtml(value);
}
