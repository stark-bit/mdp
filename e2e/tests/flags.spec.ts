// Command-line flags and the config file, each checked by what it changes in
// the page or in the processes the server starts.
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type Server as NetServer } from "node:net";
import { tmpdir } from "node:os";
import { networkInterfaces } from "node:os";
import { join } from "node:path";

import type { Page } from "@playwright/test";

import type { Server, StartOptions } from "../lib/server";
import { expect, preview, test, token } from "../lib/test";

const body = (page: Page) => page.locator(".markdown-body");

// Ports for these tests come from a range per worker, below the ones the OS
// hands out, so parallel tests cannot pick the same port: macOS lets one test
// listen on 127.0.0.1 and another on :: with the same port.
const PORTS_PER_WORKER = 500;

async function listen(port: number, host?: string): Promise<NetServer | undefined> {
  const server = createServer();
  return new Promise((resolve) => {
    server.once("error", () => resolve(undefined));
    server.listen(port, host, () => resolve(server));
  });
}

/** Occupies a free port until the test ends; all interfaces unless `host` is given. */
async function occupyPort(host?: string): Promise<{ port: number; server: NetServer }> {
  const base = 20_000 + test.info().parallelIndex * PORTS_PER_WORKER;
  // leave room for the server's fallback to the next 20 ports
  for (let attempt = 0; attempt < 50; attempt++) {
    const port = base + Math.floor(Math.random() * (PORTS_PER_WORKER - 40));
    // free on every interface, whatever we then listen on
    const probe = await listen(port);
    if (!probe) continue;
    await new Promise((r) => probe.close(r));
    const server = await listen(port, host);
    if (server) return { port, server };
  }
  throw new Error("no free port for the test");
}

async function freePort(): Promise<number> {
  const { port, server } = await occupyPort();
  await new Promise((r) => server.close(r));
  return port;
}

function portOf(url: string) {
  return Number(new URL(url).port);
}

test.describe("--port", () => {
  test("is used when it is free", async ({ launch, page }) => {
    const port = await freePort();
    const server = await launch({ files: ["note.md"], flags: ["--port", String(port)] });
    expect(portOf(server.url)).toBe(port);
    await preview(server, page, "note.md", "# Port\n");
    await expect(body(page).locator("h1")).toContainText("Port");
  });

  test("falls back to the next port when it is taken", async ({ launch, page }) => {
    const { port, server: occupier } = await occupyPort("127.0.0.1");
    try {
      const server = await launch({ files: ["note.md"], flags: ["--port", String(port)] });
      expect(portOf(server.url)).toBeGreaterThan(port);
      expect(portOf(server.url)).toBeLessThan(port + 20);
      await preview(server, page, "note.md", "# Taken\n");
      await expect(body(page).locator("h1")).toContainText("Taken");
    } finally {
      await new Promise((r) => occupier.close(r));
    }
  });

  // A dev server listening on all interfaces, as Node does by default. The
  // preview only binds 127.0.0.1, which macOS allows next to it, but the URL
  // says localhost and browsers try ::1 first: they get the other server.
  test("a port another program listens on is not used", async ({ launch, page }) => {
    const { port, server: occupier } = await occupyPort();
    try {
      const server = await launch({ files: ["note.md"], flags: ["--port", String(port)] });
      const title = token();
      await preview(server, page, "note.md", `# ${title}\n`);
      await expect(body(page).locator("h1")).toContainText(title);
      expect(portOf(server.url)).not.toBe(port);
    } finally {
      await new Promise((r) => occupier.close(r));
    }
  });

  test("two servers with the same port each get a working preview", async ({ launch, page, context }) => {
    const port = await freePort();
    const first = await launch({ files: ["a.md"], flags: ["--port", String(port)] });
    const second = await launch({ files: ["b.md"], flags: ["--port", String(port)] });
    const a = token();
    const b = token();
    await preview(first, page, "a.md", `# ${a}\n`);
    const other = await context.newPage();
    await preview(second, other, "b.md", `# ${b}\n`);
    expect(portOf(first.url)).not.toBe(portOf(second.url));
    await expect(body(page).locator("h1")).toContainText(a);
    await expect(body(other).locator("h1")).toContainText(b);
  });
});

test.describe("--root", () => {
  test("confines urls to the root directory", async ({ launch, page }) => {
    const server = await launch({
      files: ["docs/note.md"],
      flags: ["--root", "docs"],
      fixtures: { "docs/note.md": "# Inside\n", "secret.md": "# secret\n" },
    });
    expect(server.url).toMatch(/\/files\/note\.md$/);
    await page.goto(server.url);
    await expect(body(page).locator("h1")).toContainText("Inside");

    const origin = new URL(server.url).origin;
    for (const path of ["/files/..%2Fsecret.md", "/files/secret.md", "/files/%2E%2E%2Fsecret.md"]) {
      const res = await page.request.get(origin + path);
      expect(res.status(), path).toBe(404);
    }
  });
});

test.describe("opening the browser", () => {
  /**
   * A root with fake `open`, `xdg-open` and `fakebrowser` commands on the
   * server's PATH, which append their arguments to the returned log.
   */
  async function withFakeOpener(launch: (o: StartOptions) => Promise<Server>, options: StartOptions = {}) {
    const dir = realpathSync(mkdtempSync(join(tmpdir(), "mkdp-open-")));
    const log = join(dir, "opened");
    mkdirSync(join(dir, "bin"));
    for (const name of ["open", "xdg-open", "fakebrowser"]) {
      const script = join(dir, "bin", name);
      writeFileSync(script, `#!/bin/sh\necho "${name} $*" >> '${log}'\n`);
      chmodSync(script, 0o755);
    }
    const server = await launch({
      cwd: dir,
      noOpen: false,
      env: { PATH: `${join(dir, "bin")}:${process.env.PATH ?? ""}` },
      ...options,
    });
    return { server, log, dir };
  }

  test("uses the system opener by default", async ({ launch }) => {
    test.skip(process.platform === "win32", "cmd.exe cannot be faked");
    const { log, dir } = await withFakeOpener(launch, { files: ["note.md"] });
    try {
      const opener = process.platform === "darwin" ? "open" : "xdg-open";
      const line = await readWhenPresent(log);
      expect(line).toMatch(new RegExp(`^${opener} http://localhost:\\d+/files/note\\.md$`));
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("--no-open does not open a browser", async ({ launch }) => {
    test.skip(process.platform === "win32", "cmd.exe cannot be faked");
    const { log, dir } = await withFakeOpener(launch, { files: ["note.md"], flags: ["--no-open"] });
    try {
      await new Promise((r) => setTimeout(r, 1_000));
      expect(readWhenPresentNow(log)).toBe("");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("--browser names the browser", async ({ launch }) => {
    test.skip(process.platform === "win32", "cmd.exe cannot be faked");
    const { log, dir } = await withFakeOpener(launch, { files: ["note.md"], flags: ["--browser", "fakebrowser"] });
    try {
      const line = await readWhenPresent(log);
      // macOS passes the browser to `open -a`, other systems run it directly
      expect(line).toMatch(
        process.platform === "darwin"
          ? /^open -a fakebrowser http:\/\/localhost:\d+\/files\/note\.md$/
          : /^fakebrowser http:\/\/localhost:\d+\/files\/note\.md$/,
      );
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("--browser as a list runs that command with its arguments", async ({ launch }) => {
    test.skip(process.platform === "win32", "the fake browser is a shell script");
    const argv = JSON.stringify(["fakebrowser", "-P", "work profile"]);
    const { log, dir } = await withFakeOpener(launch, { files: ["note.md"], flags: ["--browser", argv] });
    try {
      const line = await readWhenPresent(log);
      expect(line).toMatch(/^fakebrowser -P work profile http:\/\/localhost:\d+\/files\/note\.md$/);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

function readWhenPresentNow(file: string): string {
  try {
    return readFileSync(file, "utf8").trim();
  } catch {
    return "";
  }
}

/** Waits until the fake opener log has content, and returns it. */
async function readWhenPresent(file: string): Promise<string> {
  await expect.poll(() => readWhenPresentNow(file), { timeout: 10_000 }).not.toBe("");
  return readWhenPresentNow(file);
}

test.describe("open_to_the_world in the config file", () => {
  const lanIp = Object.values(networkInterfaces())
    .flat()
    .find((a) => a && a.family === "IPv4" && !a.internal)?.address;

  for (const open of [false, true]) {
    test.describe(`= ${open}`, () => {
      test.use({ config: { open_to_the_world: open }, files: ["note.md"] });

      test(open ? "listens on the network" : "listens on localhost only", async ({ server, page }) => {
        test.skip(!lanIp, "this machine has no network address to connect to");
        await preview(server, page, "note.md", "# World\n");
        await expect(body(page).locator("h1")).toContainText("World");
        const status = (path: string) =>
          fetch(`http://${lanIp}:${portOf(server.url)}${path}`).then(
            (r) => r.status,
            () => "refused",
          );
        const share = new URL(server.url).searchParams.get("token");
        expect(await status(`/files/note.md?token=${share}`)).toBe(open ? 200 : "refused");
        // without the token from the url, the network sees nothing
        expect(await status("/files/note.md")).toBe(open ? 401 : "refused");
        expect(await status("/files/note.md?token=guess")).toBe(open ? 401 : "refused");
        if (open) expect(share).toMatch(/^[0-9a-f]{32}$/);
        else expect(share).toBeNull();
        if (open) expect(new URL(server.url).hostname).toBe(lanIp);
      });
    });
  }
});
