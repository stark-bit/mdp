// The server process over its life: a clean exit on Ctrl-C, and a page that
// notices when the server dies and reconnects to a new one.
import { createServer, type Server as NetServer } from "node:net";

import { Server } from "../lib/server";
import { expect, preview, test, token } from "../lib/test";

/** A port that is free right now on every interface. */
async function freePort(): Promise<number> {
  const probe: NetServer = createServer();
  await new Promise<void>((r) => probe.listen(0, "127.0.0.1", r));
  const port = (probe.address() as { port: number }).port;
  await new Promise((r) => probe.close(r));
  return port;
}

test("Ctrl-C stops the server with exit code 0", async ({ launch }) => {
  const server = await launch({ files: ["note.md"] });
  const url = new URL(server.url).origin;
  server.process.kill("SIGINT");
  expect(await server.exited).toBe(0);
  await expect
    .poll(() => fetch(url).then(() => "answered", () => "refused"), { timeout: 5_000 })
    .toBe("refused");
});

test("a killed server shows as disconnected in the page", async ({ server, page }) => {
  await preview(server, page, "note.md", "# Crash\n");
  await expect(page.locator(".markdown-body h1")).toContainText("Crash");
  server.process.kill("SIGKILL");
  await expect(page.locator("#page-header .status")).toHaveText("Disconnected");
  // the last render stays readable
  await expect(page.locator(".markdown-body h1")).toContainText("Crash");
});

test("the page reconnects to a new server on the same port", async ({ launch, page }) => {
  const port = await freePort();
  const server = await launch({ files: ["note.md"], flags: ["--port", String(port)] });
  await preview(server, page, "note.md", "# Reconnect\n");
  await expect(page.locator(".markdown-body h1")).toContainText("Reconnect");
  server.process.kill("SIGKILL");
  await expect(page.locator("#page-header .status")).toHaveText("Disconnected");

  const again = await Server.start({
    cwd: server.dir,
    files: ["note.md"],
    flags: ["--port", String(port)],
  });
  try {
    // the page keeps retrying, and the new server answers on the same port
    await expect(page.locator("#page-header .status")).toHaveCount(0);
    await expect(page.locator(".markdown-body h1")).toContainText("Reconnect");
  } finally {
    await again.stop();
  }
});

test("each file has its own page", async ({ launch, page, context }) => {
  const a = token();
  const b = token();
  const server = await launch({ files: ["a.md", "b.md"] });
  await preview(server, page, "a.md", `# ${a}\n`);
  const other = await context.newPage();
  await preview(server, other, "b.md", `# ${b}\n`);
  await expect(page.locator(".markdown-body h1")).toContainText(a);
  await expect(other.locator(".markdown-body h1")).toContainText(b);

  // a change to b only reaches b's page
  const inB = token();
  server.write("b.md", `# ${b}\n\n${inB}\n`);
  await expect(other.locator(".markdown-body")).toContainText(inB);
  await expect(page.locator(".markdown-body")).not.toContainText(inB);

  // and a change to a only reaches a's page
  const inA = token();
  server.write("a.md", `# ${a}\n\n${inA}\n`);
  await expect(page.locator(".markdown-body")).toContainText(inA);
  await expect(other.locator(".markdown-body")).not.toContainText(inA);
});

test("reloading the page shows the current file", async ({ server, page }) => {
  await preview(server, page, "note.md", "# Reload\n");
  const written = token();
  server.write("note.md", `# Reload\n\n${written}\n`);
  await expect(page.locator(".markdown-body")).toContainText(written);
  await page.reload();
  await expect(page.locator(".markdown-body")).toContainText(written);
});
