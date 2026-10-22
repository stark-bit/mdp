// Messages exchanged with the preview server over the /ws WebSocket.
import type { KatexOptions } from "katex";
import type { MarkdownItOptions } from "markdown-it";

import type { UmlOptions } from "./markdown/plantuml";

/** Options sent by the server */
export interface PreviewOptions {
  mkit?: MarkdownItOptions;
  katex?: KatexOptions;
  uml?: UmlOptions;
  maid?: Record<string, unknown>;
  front_matter?: "hide" | "panel" | "raw";
  hide_yaml_meta?: number;
  sequence_diagrams?: Record<string, unknown>;
  flowchart_diagrams?: Record<string, unknown>;
  content_editable?: number | boolean;
  disable_filename?: number | boolean;
  toc?: Record<string, unknown>;
}

/** Page data sent by the server */
export interface PreviewData {
  options: PreviewOptions;
  pageTitle: string;
  theme: string;
  name: string;
  content: string[];
}

export type ServerMessage = { type: "refresh_content"; data: PreviewData };
