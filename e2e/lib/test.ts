import { randomBytes } from "node:crypto";

import { test as base, type Page } from "@playwright/test";

import { Server, type StartOptions } from "./server";


export { expect } from "@playwright/test";
export { waitFor } from "./server";

/** A word that cannot be in the page unless it came from this test's file. */
export function token() {
  return `tok${randomBytes(6).toString("hex")}`;
}

export const test = base.extend<{
  /** files the server previews at startup, set per project or per test */
  files: string[];
  /** keys for the server's config file, set per test */
  config: Record<string, unknown> | undefined;
  /** starts more servers in fresh temp roots, stopped at the end of the test */
  launch: (options?: StartOptions) => Promise<Server>;
  /** one server in a fresh temp root, stopped at the end of the test */
  server: Server;
  /** browser console output and page errors, attached to failing tests */
  consoleLog: string[];
}>({
  files: [[], { option: true }],
  config: [undefined, { option: true }],
  consoleLog: async ({ context }, use) => {
    const log: string[] = [];
    const watch = (page: Page) => {
      page.on("console", (msg) => log.push(`[${msg.type()}] ${page.url()}: ${msg.text()}`));
      page.on("pageerror", (error) => log.push(`[pageerror] ${page.url()}: ${error.stack ?? error.message}`));
    };
    context.pages().forEach(watch);
    context.on("page", watch);
    await use(log);
  },
  launch: async ({ consoleLog }, use, testInfo) => {
    const servers: Server[] = [];
    try {
      await use(async (options) => {
        // previewing one file by default: the server only prints urls for
        // files it was started with, and every test needs a url
        const server = await Server.start({ files: ["note.md"], ...options });
        servers.push(server);
        return server;
      });
    } finally {
      const failed = testInfo.status !== testInfo.expectedStatus;
      for (const [i, server] of servers.entries()) {
        if (failed) {
          await testInfo.attach(`server ${i + 1}`, { body: server.log(), contentType: "text/plain" });
        }
        await server.stop();
      }
      if (failed && consoleLog.length) {
        await testInfo.attach("page console", { body: consoleLog.join("\n"), contentType: "text/plain" });
      }
    }
  },
  server: async ({ launch, files, config }, use) => {
    const options: StartOptions = { config };
    if (files.length) options.files = files;
    await use(await launch(options));
  },
});

/** Writes a markdown file into the server's root and loads its page. */
export async function preview(server: Server, page: Page, path: string, content: string) {
  server.write(path, content);
  const url = server.pageUrl(path);
  await page.goto(url);
  return url;
}
