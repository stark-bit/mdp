// Release checks that need neither a browser nor a running server: the names
// and versions spread over the workflow and the built binary must agree, or a
// release publishes something users cannot install.
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { expect, test } from "@playwright/test";

import { REPO, SERVER_BINARY } from "../lib/server";
import { VERSION } from "../lib/release";

const read = (path: string) => readFileSync(join(REPO, path), "utf8");

/** Asset base names the release workflow publishes. */
function releasedAssets() {
  const workflow = read(".github/workflows/release.yml");
  const names = [...workflow.matchAll(/^\s+name: (markdown-preview-[\w-]+)$/gm)].map((m) => m[1]);
  expect(names.length, "release.yml matrix names").toBeGreaterThan(0);
  return names;
}

test.describe("versions", () => {
  test("Cargo.toml, Cargo.lock and the built binary agree", () => {
    const lock = /name = "markdown-preview"\nversion = "([^"]+)"/.exec(read("Cargo.lock"));
    expect(lock?.[1], "Cargo.lock").toBe(VERSION);
    expect(execFileSync(SERVER_BINARY, ["--version"], { encoding: "utf8" }).trim(), "binary").toBe(VERSION);
  });

  test("CHANGELOG.md has an entry for the version", () => {
    const top = /^## (v\S+)/m.exec(read("CHANGELOG.md"))?.[1];
    expect(top, "newest CHANGELOG.md entry").toBe(`v${VERSION}`);
  });

  test("release.sh tags what release.yml builds", () => {
    expect(read("release.sh")).toContain('tag="v$version"');
    expect(read(".github/workflows/release.yml")).toMatch(/tags: \["v\*"\]/);
  });
});

test.describe("release assets", () => {
  test("the archives hold the binary the release workflow builds", () => {
    const workflow = read(".github/workflows/release.yml");
    expect(workflow).toContain("cp target/${{ matrix.target }}/release/markdown-preview ${{ matrix.name }}");
    expect(workflow).toContain("tar -czf ${{ matrix.name }}.tar.gz ${{ matrix.name }}");
    expect(workflow).toContain("markdown-preview.exe ${{ matrix.name }}.exe");
  });
});

const CONTENT = "docs/content/docs";
const SITE = "https://mkdp.sammaji.com";

/** The markdown files users read: the site's pages and the repository's own. */
function docPages() {
  const pages = ["README.md", "CONTRIBUTING.md"];
  const walk = (dir: string) => {
    for (const entry of readdirSync(join(REPO, dir), { withFileTypes: true })) {
      const path = `${dir}/${entry.name}`;
      if (entry.isDirectory()) walk(path);
      else if (entry.name.endsWith(".mdx")) pages.push(path);
    }
  };
  walk(CONTENT);
  return pages.filter((page) => existsSync(join(REPO, page)));
}

/** Markdown without its fenced code blocks, where `#` and `](` mean nothing. */
const prose = (markdown: string) => markdown.replace(/^(`{3,})[\s\S]*?^\1$/gm, "");

/** The anchors the site (and GitHub) give a page's headings. */
function anchors(markdown: string) {
  const seen = new Map<string, number>();
  return [...prose(markdown).matchAll(/^#{1,6} (.+)$/gm)].map((m) => {
    const slug = m[1]
      .trim()
      .toLowerCase()
      .replace(/[^\p{L}\p{N} _-]/gu, "")
      .replace(/ /g, "-");
    const n = seen.get(slug) ?? 0;
    seen.set(slug, n + 1);
    return n ? `${slug}-${n}` : slug;
  });
}

/** The content file of a site path such as `/features/themes/` or `/docs/features/themes/`. */
function pageOfUrl(path: string) {
  const slug = path.replace(/^\/docs\/?/, "").replace(/\/$/, "");
  return [`${CONTENT}/${slug || "index"}.mdx`, `${CONTENT}/${slug}/index.mdx`].find((f) => existsSync(join(REPO, f)));
}

/** The repository file a link in `doc` points to, or undefined when it is broken. */
function linkTarget(doc: string, file: string) {
  if (file.startsWith(SITE)) return file === SITE || file === `${SITE}/` ? "docs/app/(home)/page.tsx" : pageOfUrl(file.slice(SITE.length));
  // site pages live at the root; /docs/ links are redirected there
  if (file.startsWith("/")) return pageOfUrl(file);
  const path = file ? join(dirname(doc), decodeURIComponent(file)) : doc;
  return existsSync(join(REPO, path)) ? path : undefined;
}

test.describe("examples", () => {
  test("the files examples/features.md shows are in the repository", () => {
    const tracked = new Set(execFileSync("git", ["ls-files", "examples"], { cwd: REPO, encoding: "utf8" }).split("\n"));
    const example = readFileSync(join(REPO, "examples/features.md"), "utf8");
    const srcs = [...example.matchAll(/!\[[^\]]*\]\(([^)\s]+)|<img\s[^>]*src="([^"]+)"/g)]
      .map((m) => m[1] ?? m[2])
      .filter((src) => !/^[a-z]+:|^\/\//.test(src));
    expect(srcs.length).toBeGreaterThan(0);
    for (const src of srcs) {
      expect(tracked, `${src}, e.g. ignored by .gitignore`).toContain(join("examples", src));
    }
  });
});

test.describe("docs", () => {
  test("links between the docs point to pages and headings that exist", () => {
    let checked = 0;
    for (const doc of docPages()) {
      const text = prose(read(doc));
      // markdown links, and <Card href="..."> in MDX
      const targets = [...text.matchAll(/\]\(([^)\s]+)\)|href="([^"]+)"/g)].map((m) => m[1] ?? m[2]);
      for (const target of targets) {
        if (/^[a-z]+:/.test(target) && !target.startsWith(SITE)) continue;
        const [file, anchor] = target.split("#");
        const path = linkTarget(doc, file);
        expect(path, `${target} in ${doc}`).toBeDefined();
        if (anchor && /\.mdx?$/.test(path!)) {
          expect(anchors(read(path!)), `${target} in ${doc}`).toContain(anchor);
        }
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(50);
  });
});
