import { afterEach, describe, expect, test } from "vitest";

import { displayName, pageFromUrl } from "@/components/preview";

describe("displayName", () => {
  test.each([
    ["/home/me/notes/README.md", "README"],
    ["C:\\Users\\me\\notes\\todo.markdown", "todo"],
    ["relative/dir/file.tar.md", "file.tar"],
    ["no-extension", "no-extension"],
    ["/home/me/.hidden", ".hidden"],
    ["", ""],
  ])("%s -> %s", (path, name) => {
    expect(displayName(path)).toBe(name);
  });
});

describe("pageFromUrl", () => {
  afterEach(() => window.history.replaceState(null, "", "/"));

  test.each([
    ["/files/README.md", "/files/README.md"],
    ["/files/docs/my%20file.md", "/files/docs/my%20file.md"],
    ["/files/README.md?x=1#top", "/files/README.md"],
    ["/files/docs/my%20file.md#usage", "/files/docs/my%20file.md"],
  ])("%s -> %s", (path, page) => {
    window.history.replaceState(null, "", path);
    expect(pageFromUrl()).toBe(page);
  });

  test.each(["/", "/files/", "/files", "/page/1", "/other/3"])("%s is no preview", (path) => {
    window.history.replaceState(null, "", path);
    expect(pageFromUrl()).toBeNull();
  });
});
