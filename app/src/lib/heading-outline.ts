export interface HeadingEntry {
  id: string;
  level: number;
  title: string;
  element: HTMLHeadingElement;
}

function createSlug(title: string): string {
  return title
    .normalize("NFKD")
    .toLowerCase()
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^\p{Letter}\p{Number}]+/gu, "-")
    .replace(/^-|-$/g, "") || "heading";
}

export function extractHeadings(root: ParentNode): HeadingEntry[] {
  const headings = Array.from(
    root.querySelectorAll<HTMLHeadingElement>("h1, h2, h3, h4, h5, h6"),
  );
  const usedIds = new Set(
    headings.map((heading) => heading.id).filter(Boolean),
  );

  return headings.map((element) => {
    const title = element.textContent?.trim() ?? "";
    if (!element.id) {
      const base = createSlug(title);
      let id = base;
      let suffix = 1;
      while (usedIds.has(id)) {
        id = `${base}-${suffix}`;
        suffix += 1;
      }
      element.id = id;
      usedIds.add(id);
    }

    return {
      id: element.id,
      level: Number(element.tagName.slice(1)),
      title,
      element,
    };
  });
}

export const MAX_DEPTH = 6;

export function filterByDepth(
  headings: HeadingEntry[],
  depth: number,
): HeadingEntry[] {
  return headings.filter((heading) => heading.level <= depth);
}

export function parseDepth(value: string | null): number | null {
  const depth = Number(value);
  return Number.isInteger(depth) && depth >= 1 && depth <= MAX_DEPTH
    ? depth
    : null;
}

export function deepestLevel(headings: HeadingEntry[]): number {
  return Math.max(1, ...headings.map((heading) => heading.level));
}
