// The flow a user goes through: run the server on a file, edit the file on
// disk, watch the page update. Every assertion looks for content that only
// exists if the feature worked, usually a random token from the file.
import { expect, preview, test, token } from "../lib/test";

test.use({ files: ["note.md"] });

test("renders the file in the page the server opens", async ({ server, page }) => {
  const title = token();
  const body = token();
  await preview(server, page, "note.md", `# ${title}\n\nsome *${body}* text\n`);

  // named after the file, relative to the server's root
  expect(server.url).toMatch(/^http:\/\/localhost:\d+\/files\/note\.md$/);
  await expect(page.locator(".markdown-body h1")).toContainText(title);
  await expect(page.locator(".markdown-body em")).toHaveText(body);
  await expect(page.locator("#page-header h3")).toHaveText("note");
});

test("shows edits written to disk, without reloading", async ({ server, page }) => {
  await preview(server, page, "note.md", "# Title\n");
  await expect(page.locator(".markdown-body h1")).toContainText("Title");

  const first = token();
  server.write("note.md", `# Title\n\n## ${first}\n`);
  await expect(page.locator(".markdown-body h2")).toContainText(first);

  const second = token();
  server.write("note.md", `# Title\n\n## ${first}\n\n- ${second}\n`);
  await expect(page.locator(".markdown-body li")).toHaveText(second);
});

test("shows images next to the markdown file", async ({ server, page }) => {
  // an unusual size, so a broken image or the wrong file cannot match it
  server.write(
    "images/pic.svg",
    '<svg xmlns="http://www.w3.org/2000/svg" width="37" height="23"><rect width="37" height="23" fill="red"/></svg>',
  );
  await preview(server, page, "note.md", "# Images\n\n![pic](images/pic.svg)\n");

  const img = page.locator(".markdown-body img[alt=pic]");
  await expect(img).toBeVisible();
  await expect.poll(() => img.evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(37);
  expect(await img.evaluate((el: HTMLImageElement) => el.naturalHeight)).toBe(23);
});

test.describe("file urls", () => {
  test("a file in a folder has its path in the url", async ({ server, page }) => {
    const title = token();
    await preview(server, page, "docs/my notes.md", `# ${title}\n`);
    await expect(page).toHaveURL(/\/files\/docs\/my%20notes\.md$/);
    await expect(page.locator(".markdown-body h1")).toContainText(title);
  });

  test("a link to another markdown file opens it from disk (#58)", async ({ server, page }) => {
    const title = token();
    server.write("docs/other.md", `# ${title}\n\n![logo](img/logo.svg)\n`);
    server.write(
      "docs/img/logo.svg",
      '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>',
    );
    await preview(server, page, "note.md", "# Note\n\n[other](docs/other.md)\n");
    await page.locator(".markdown-body a", { hasText: "other" }).click();

    await expect(page).toHaveURL(/\/files\/docs\/other\.md$/);
    await expect(page.locator(".markdown-body h1")).toContainText(title);
    await expect(page.locator("#page-header h3")).toHaveText("other");
    // images are relative to the linked file
    await expect(page.locator(".markdown-body img")).toHaveJSProperty("naturalWidth", 10);
  });

  test("urls cannot reach files outside the working directory", async ({ server, page }) => {
    const secret = server.write("secret.md", "# secret\n");
    await preview(server, page, "note.md", "# Note\n");
    await expect(page.locator(".markdown-body h1")).toContainText("Note");

    const origin = new URL(server.url).origin;
    for (const path of ["/files/..%2Fsecret.md", "/files/..%5Csecret.md", `/files/${encodeURIComponent(secret)}`]) {
      const res = await page.request.get(origin + path);
      expect(res.status(), path).toBe(404);
    }
  });
});
