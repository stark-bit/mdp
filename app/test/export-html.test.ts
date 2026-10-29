import { afterEach, describe, expect, test, vi } from "vitest";

import { exportHtml } from "@/lib/export-html";

function sheet(link: HTMLLinkElement, css: string) {
  Object.defineProperty(link, "sheet", {
    configurable: true,
    value: { cssRules: [{ cssText: css }] },
  });
}

afterEach(() => {
  vi.restoreAllMocks();
  document.head.innerHTML = "";
  document.body.innerHTML = "";
});

describe("exportHtml", () => {
  test("inlines stylesheet assets relative to each stylesheet", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const requestedUrl = String(input);
      if (requestedUrl.endsWith("/media/x.woff2")) {
        return new Response("fontdata", {
          headers: { "content-type": "font/woff2" },
        });
      }
      if (requestedUrl.endsWith("/_static/sequence.woff2")) {
        return new Response("sequence", {
          headers: { "content-type": "font/woff2" },
        });
      }
      if (requestedUrl === "https://fonts.example.test/theme.woff2") {
        return new Response("themefont", {
          headers: { "content-type": "font/woff2" },
        });
      }
      if (requestedUrl.endsWith("/fonts/custom.woff2")) {
        return new Response("customfont", {
          headers: { "content-type": "font/woff2" },
        });
      }
      throw new Error(`Unexpected asset request: ${requestedUrl}`);
    });

    const katex = document.createElement("link");
    katex.rel = "stylesheet";
    katex.href = "/_next/static/chunks/katex.css";
    sheet(katex, '.katex { src: url("../media/x.woff2"); }');
    const sequence = document.createElement("link");
    sequence.rel = "stylesheet";
    sequence.href = "/_static/sequence.css";
    sheet(
      sequence,
      '.sequence { src: url(/_static/sequence.woff2); } .theme { src: url(https://fonts.example.test/theme.woff2); } .custom { src: url(/fonts/custom.woff2); }',
    );
    document.head.append(katex, sequence);

    const html = await exportHtml(document.createElement("section"), {
      title: "Page",
      theme: "light",
      lang: "en",
    });

    expect(fetchMock.mock.calls.map(([url]) => String(url))).toEqual([
      new URL("../media/x.woff2", katex.href).href,
      new URL("/_static/sequence.woff2", sequence.href).href,
      "https://fonts.example.test/theme.woff2",
      new URL("/fonts/custom.woff2", sequence.href).href,
    ]);
    expect(html).toContain("data:font/woff2;base64,Zm9udGRhdGE=");
    expect(html).toContain("data:font/woff2;base64,c2VxdWVuY2U=");
    expect(html).toContain("data:font/woff2;base64,dGhlbWVmb250");
    expect(html).toContain("data:font/woff2;base64,Y3VzdG9tZm9udA==");
    fetchMock.mockRestore();
  });

  test("inlines images and exports live chart canvases", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      if (String(input) === "http://localhost:3000/local.svg") {
        return new Response("<svg/>", {
          headers: { "content-type": "image/svg+xml" },
        });
      }
      throw new Error("offline");
    });
    const body = document.createElement("section");
    for (const [src, alt] of [
      ["/local.svg", "local"],
      ["/remote.svg", "remote"],
      ["data:image/png;base64,AA==", "embedded"],
    ]) {
      const image = document.createElement("img");
      image.src = src;
      image.alt = alt;
      body.append(image);
    }
    const chartWrapper = document.createElement("div");
    chartWrapper.className = "chartjs";
    const canvas = document.createElement("canvas");
    canvas.width = 320;
    canvas.height = 180;
    chartWrapper.append(canvas);
    body.append(chartWrapper, document.createElement("canvas"));
    vi.spyOn(canvas, "toDataURL").mockReturnValue("data:image/png;base64,Y2hhcnQ=");

    const html = await exportHtml(body, { title: "Page", theme: "light", lang: "en" });
    const result = new DOMParser().parseFromString(html, "text/html");

    expect(result.querySelector<HTMLImageElement>('img[alt="local"]')?.src).toBe(
      "data:image/svg+xml;base64,PHN2Zy8+",
    );
    expect(result.querySelector('img[alt="remote"]')?.getAttribute("src")).toBe(
      "/remote.svg",
    );
    expect(result.querySelector('img[alt="embedded"]')?.getAttribute("src")).toBe(
      "data:image/png;base64,AA==",
    );
    const chart = result.querySelector<HTMLImageElement>(".chartjs img");
    expect(chart?.src).toBe("data:image/png;base64,Y2hhcnQ=");
    expect(chart?.getAttribute("width")).toBe("320");
    expect(result.querySelector("body > main canvas")).not.toBeNull();
    expect(fetchMock.mock.calls.map(([url]) => String(url))).toEqual([
      "http://localhost:3000/local.svg",
      "http://localhost:3000/remote.svg",
    ]);
  });

  test("keeps fragment, data, and failed URLs unchanged", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockRejectedValue(
      new Error("offline"),
    );
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = "/_static/theme.css";
    sheet(
      link,
      'a { mask: url(#shape); } b { src: url(data:font/woff2;base64,AA==); } c { src: url(https://other.test/font.woff2); } d { src: url(missing.woff2); }',
    );
    document.head.append(link);

    const html = await exportHtml(document.createElement("section"), {
      title: "Page",
      theme: "light",
      lang: "en",
    });

    expect(html).toContain("url(#shape)");
    expect(html).toContain("url(data:font/woff2;base64,AA==)");
    expect(html).toContain("url(https://other.test/font.woff2)");
    expect(html).toContain("url(missing.woff2)");
    expect(fetchMock.mock.calls.map(([url]) => String(url))).toEqual([
      "https://other.test/font.woff2",
      new URL("missing.woff2", link.href).href,
    ]);
    fetchMock.mockRestore();
  });

  test("inlines styles and keeps rendered theme and content", async () => {
    const first = document.createElement("link");
    first.rel = "stylesheet";
    first.href = "/_static/markdown.css";
    sheet(first, ".markdown-body { color: red; }");
    const second = document.createElement("link");
    second.rel = "stylesheet";
    second.href = "/_theme/theme.css";
    sheet(second, ".markdown-body { color: blue; }");
    document.head.append(first, second);

    const body = document.createElement("section");
    body.className = "markdown-body";
    body.setAttribute("contenteditable", "true");
    const details = document.createElement("details");
    details.open = true;
    const summary = document.createElement("summary");
    summary.textContent = "Open details";
    const paragraph = document.createElement("p");
    paragraph.textContent = "Rendered text";
    details.append(summary, paragraph);
    body.append(details);
    for (const [tag, className, text] of [
      ["button", "diagram-open", "Open diagram"],
      ["div", "diagram-viewer", "Viewer"],
      ["script", "", "window.bad = true"],
    ]) {
      const element = document.createElement(tag);
      if (className) element.className = className;
      element.textContent = text;
      body.append(element);
    }
    const editable = document.createElement("p");
    editable.setAttribute("contenteditable", "true");
    editable.textContent = "Editable text";
    body.append(editable);

    const html = await exportHtml(body, {
      title: 'Preview "name"',
      theme: "dark",
      lang: "en",
    });
    const result = new DOMParser().parseFromString(html, "text/html");

    expect(html).toMatch(/^<!doctype html>/i);
    expect(result.title).toBe('Preview "name"');
    expect(result.documentElement.lang).toBe("en");
    expect(result.documentElement.classList.contains("dark")).toBe(true);
    expect(result.documentElement.dataset.theme).toBe("dark");
    expect(result.querySelectorAll("style")).toHaveLength(2);
    expect(result.querySelectorAll("style")[0].textContent).toContain("color: red");
    expect(result.querySelectorAll("style")[1].textContent).toContain("color: blue");
    expect(result.querySelector("details")?.hasAttribute("open")).toBe(true);
    expect(result.querySelector(".markdown-body")?.textContent).toContain("Rendered text");
    expect(result.querySelector(".diagram-open, .diagram-viewer, script")).toBeNull();
    expect(result.querySelector("[contenteditable]")).toBeNull();
    expect(result.querySelector("header, .header-actions, .theme-toggle")).toBeNull();
    expect(result.querySelectorAll("link[rel=stylesheet]")).toHaveLength(0);
  });

  test("does not embed stylesheets from other origins", async () => {
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = "https://example.test/theme.css";
    sheet(link, "body { color: red; }");
    document.head.append(link);
    const body = document.createElement("section");

    const html = await exportHtml(body, { title: "Page", theme: "light", lang: "en" });

    expect(html).not.toContain("color: red");
  });
});
