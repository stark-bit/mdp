import { describe, expect, test } from "vitest";

import {
  deepestLevel,
  extractHeadings,
  filterByDepth,
  parseDepth,
} from "@/lib/heading-outline";

describe("extractHeadings", () => {
  test("extracts headings and adds unique ids", () => {
    const root = document.createElement("section");
    root.innerHTML = `
      <p>Ignored text</p>
      <h1 id="existing-anchor">Main title</h1>
      <h3>Nested title</h3>
      <h3>Nested title</h3>
      <h6>Deep title</h6>
    `;

    const headings = extractHeadings(root);

    expect(headings.map(({ id, level, title }) => ({ id, level, title }))).toEqual([
      { id: "existing-anchor", level: 1, title: "Main title" },
      { id: "nested-title", level: 3, title: "Nested title" },
      { id: "nested-title-1", level: 3, title: "Nested title" },
      { id: "deep-title", level: 6, title: "Deep title" },
    ]);
    expect(root.querySelector("h3")?.id).toBe("nested-title");
  });
});

describe("depth helpers", () => {
  test("filters headings by depth", () => {
    const root = document.createElement("section");
    root.innerHTML = "<h1>A</h1><h2>B</h2><h5>C</h5><h6>D</h6>";
    const headings = extractHeadings(root);

    expect(filterByDepth(headings, 6)).toHaveLength(4);
    expect(filterByDepth(headings, 5).map((h) => h.title)).toEqual([
      "A",
      "B",
      "C",
    ]);
    expect(filterByDepth(headings, 1).map((h) => h.title)).toEqual(["A"]);
  });

  test("parses saved depth", () => {
    expect(parseDepth("3")).toBe(3);
    expect(parseDepth(null)).toBeNull();
    expect(parseDepth("0")).toBeNull();
    expect(parseDepth("9")).toBeNull();
    expect(parseDepth("x")).toBeNull();
  });

  test("finds deepest level", () => {
    const root = document.createElement("section");
    root.innerHTML = "<h1>A</h1><h3>B</h3>";

    expect(deepestLevel(extractHeadings(root))).toBe(3);
    expect(deepestLevel([])).toBe(1);
  });
});
