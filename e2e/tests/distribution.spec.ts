// Release checks that need neither a browser nor a running server: the names
// and versions spread over the workflow, the install scripts and the built
// binary must agree, or a release publishes something users cannot install.
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { expect, test } from "@playwright/test";

import { REPO, SERVER_BINARY } from "../lib/server";
import { VERSION } from "../lib/release";

const read = (path: string) => readFileSync(join(REPO, path), "utf8");
// the download base install.sh itself uses, so the script and these checks agree
const RELEASES = /url="([^"]+releases\/download)/.exec(read("app/install.sh"))![1];

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
  test("install.sh only downloads released assets", () => {
    const assets = releasedAssets();
    const downloads = [...read("app/install.sh").matchAll(/download (markdown-preview-[\w-]+)\.tar\.gz/g)].map(
      (m) => m[1],
    );
    expect(downloads.length).toBeGreaterThan(0);
    for (const name of downloads) expect(assets).toContain(name);
  });

  test("install.cmd downloads the Windows asset", () => {
    expect(read("app/install.cmd")).toContain('$file = "markdown-preview-win.zip"');
    expect(releasedAssets()).toContain("markdown-preview-win");
  });

  test("the archives hold the binary the release workflow builds", () => {
    const workflow = read(".github/workflows/release.yml");
    expect(workflow).toContain("cp target/${{ matrix.target }}/release/markdown-preview ${{ matrix.name }}");
    expect(workflow).toContain("tar -czf ${{ matrix.name }}.tar.gz ${{ matrix.name }}");
    expect(workflow).toContain("markdown-preview.exe ${{ matrix.name }}.exe");
  });
});

test.describe("install.sh", () => {
  test.skip(process.platform === "win32", "install.sh is for unix");

  /** Runs a copy of install.sh as if on `uname -sm` = `system`, with curl faked. */
  function install(system: string, args: string[] = [], { failDownload = false } = {}) {
    const root = mkdtempSync(join(tmpdir(), "mkdp-install-"));
    try {
      cpSync(join(REPO, "app/install.sh"), join(root, "app/install.sh"));
      cpSync(join(REPO, "Cargo.toml"), join(root, "Cargo.toml"));
      const bin = join(root, "fakebin");
      mkdirSync(bin);
      // an archive holding one executable named like the requested asset
      const log = join(root, "requests");
      writeFileSync(
        join(bin, "uname"),
        `#!/bin/sh\n[ "$1" = "-sm" ] && echo '${system}' && exit 0\nexec /usr/bin/uname "$@"\n`,
      );
      writeFileSync(
        join(bin, "curl"),
        [
          "#!/bin/sh",
          'for url; do :; done',
          `echo "$url" >> '${log}'`,
          failDownload ? "echo 'curl: (22) 404' >&2; exit 22" : "",
          'name=$(basename "$url" .tar.gz)',
          'tmp=$(mktemp -d); printf "#!/bin/sh\\necho fake\\n" > "$tmp/$name"',
          'tar -czf - -C "$tmp" "$name"',
        ].join("\n"),
      );
      chmodSync(join(bin, "uname"), 0o755);
      chmodSync(join(bin, "curl"), 0o755);
      const result = spawnSync("bash", [join(root, "app/install.sh"), ...args], {
        encoding: "utf8",
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
      });
      const requests = existsSync(log) ? readFileSync(log, "utf8").trim().split("\n") : [];
      const binDir = join(root, "app/bin");
      const installed = existsSync(binDir)
        ? execFileSync("ls", [binDir], { encoding: "utf8" }).split("\n").filter((f) => f && !f.endsWith(".gz"))
        : [];
      const executable = installed.every((f) => (statSync(join(binDir, f)).mode & 0o111) !== 0);
      return { status: result.status, output: result.stdout + result.stderr, requests, installed, executable };
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }

  const supported: [string, string][] = [
    ["Linux x86_64", "markdown-preview-linux"],
    ["Darwin x86_64", "markdown-preview-macos"],
    ["Darwin arm64", "markdown-preview-macos-arm64"],
    ["Linux aarch64", "markdown-preview-linux-arm64"],
    ["Linux arm64", "markdown-preview-linux-arm64"],
    ["FreeBSD amd64", "markdown-preview-freebsd"],
  ];
  for (const [system, asset] of supported) {
    test(`${system} installs ${asset}`, () => {
      const result = install(system, ["v1.2.3"]);
      expect(result.requests).toEqual([`${RELEASES}/v1.2.3/${asset}.tar.gz`]);
      expect(result.installed).toEqual([asset]);
      expect(result.executable).toBe(true);
    });
  }

  test("without a tag it installs the built version", () => {
    const result = install("Linux x86_64");
    expect(result.requests).toEqual([`${RELEASES}/v${VERSION}/markdown-preview-linux.tar.gz`]);
  });

  // unsupported systems must not get a binary for another architecture
  for (const system of ["Linux i686", "Linux armv7l", "FreeBSD arm64", "SunOS i86pc"]) {
    test(`${system} downloads nothing`, () => {
      const result = install(system, ["v1.2.3"]);
      expect(result.requests).toEqual([]);
      expect(result.installed).toEqual([]);
      expect(result.output).toContain("No pre-built binary available");
      expect(result.status).not.toBe(0);
    });
  }

  test("a failed download leaves no binary behind", () => {
    const result = install("Linux x86_64", ["v1.2.3"], { failDownload: true });
    expect(result.requests).toHaveLength(1);
    expect(result.installed).toEqual([]);
    expect(result.output).toContain("Command failed (exit code 22)");
  });
});

const CONTENT = "docs/content/docs";
const SITE = "https://mkdp.sammaji.com";

/** The markdown files users read: the site's pages and the repository's own. */
function docPages() {
  const pages = ["README.md", "CONTRIBUTING.md", "skip-test-case.md"];
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
