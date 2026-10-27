// The release server binary, driven as a user drives it: command-line
// flags, files on disk, and the URL it prints on stdout.
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import type { Readable } from "node:stream";

export const REPO = resolve(__dirname, "../..");
export const EXE = process.platform === "win32" ? ".exe" : "";
export const SERVER_BINARY = join(REPO, "target/release", `mm${EXE}`);

export async function waitFor<T>(
  what: string,
  poll: () => Promise<T | undefined> | T | undefined,
  timeoutMs = 10_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  while (Date.now() < deadline) {
    try {
      const value = await poll();
      if (value !== undefined) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`timed out after ${timeoutMs}ms waiting for ${what}`, { cause: lastError });
}

export interface StartOptions {
  /** markdown files to preview, relative to the root directory */
  files?: string[];
  /** extra flags, e.g. ["--port", "8123"] */
  flags?: string[];
  /** keys for the config file the server reads from its working directory */
  config?: Record<string, unknown>;
  /** files written into the root before the server starts */
  fixtures?: Record<string, string | Buffer>;
  /** the browser is not opened unless this is false */
  noOpen?: boolean;
  /** the server's working directory; a fresh temp directory by default */
  cwd?: string;
  /** extra environment, e.g. a PATH with a fake browser on it */
  env?: Record<string, string>;
}

export class Server {
  private constructor(
    readonly process: ChildProcess,
    readonly dir: string,
    readonly logFile: string,
    /** every http:// line the server printed, in order */
    readonly urls: string[],
    /** resolves with the exit code, or null when a signal killed it */
    readonly exited: Promise<number | null>,
    private readonly owned: boolean,
    private readonly output: string[],
  ) {}

  static async start(options: StartOptions = {}): Promise<Server> {
    const owned = options.cwd === undefined;
    const dir = realpathSync(options.cwd ?? mkdtempSync(join(tmpdir(), "mp-server-")));
    for (const [name, content] of Object.entries(options.fixtures ?? {})) {
      const file = join(dir, name);
      mkdirSync(dirname(file), { recursive: true });
      writeFileSync(file, content);
    }
    // a missing previewed file would make the server print a path without
    // /files/, so create empty ones the tests fill in later
    for (const file of options.files ?? []) {
      const path = join(dir, file);
      if (!existsSync(path)) {
        mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, "");
      }
    }
    if (options.config) writeFileSync(join(dir, "markdown-preview.toml"), toToml(options.config));

    const logFile = join(dir, "server.log");
    // files before flags: `--browser` takes every non-flag argument that
    // follows it, so trailing file names would be eaten as browser words
    const child = spawn(
      SERVER_BINARY,
      [
        ...(options.noOpen === false ? [] : ["--no-open"]),
        ...(options.files ?? []),
        ...(options.flags ?? []),
      ],
      {
        cwd: dir,
        env: {
          ...process.env,
          MP_LOG_FILE: logFile,
          MP_LOG_LEVEL: "debug",
          ...options.env,
        },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );

    const urls: string[] = [];
    const output: string[] = [];
    let announce!: () => void;
    const printed = new Promise<void>((r) => (announce = r));
    collect(child.stdout, output, urls, announce);
    collect(child.stderr, output, null, announce);
    const exited = new Promise<number | null>((r) => child.once("exit", (code) => r(code)));

    await new Promise<void>((resolveUrl, rejectUrl) => {
      const fail = (message: string) => rejectUrl(new Error(`${message}:\n${output.join("\n")}`));
      const onExit = (code: number | null) => fail(`the server exited with ${code} before printing a url`);
      const onTimeout = setTimeout(() => fail("the server printed no url within 10s"), 10_000);
      child.once("exit", onExit);
      printed.then(() => {
        clearTimeout(onTimeout);
        child.removeListener("exit", onExit);
        resolveUrl();
      });
    });
    return new Server(child, dir, logFile, urls, exited, owned, output);
  }

  /** The first url the server printed. */
  get url(): string {
    if (this.urls.length === 0) throw new Error(`the server printed no url:\n${this.log()}`);
    return this.urls[0]!;
  }

  /** The url of the nth browser opening the server reported, 1-based. */
  previewUrl(n = 1): string {
    if (this.urls.length < n) throw new Error(`the server printed only ${this.urls.length} url(s):\n${this.log()}`);
    return this.urls[n - 1]!;
  }

  /** What the server printed and logged, for a failing test's report. */
  log(): string {
    let file = "";
    try {
      file = readFileSync(this.logFile, "utf8");
    } catch {
      file = "(unavailable)";
    }
    return `--- server output ---\n${this.output.join("\n")}\n\n--- server log ---\n${file}`;
  }

  /** Writes a file under the server's root, creating folders. */
  write(path: string, content: string | Buffer): string {
    const file = join(this.dir, path);
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, content);
    return file;
  }

  /** The page url of a file under the root, encoded like the server's. */
  pageUrl(path: string): string {
    const base = new URL(this.url);
    const segments = path
      .split(/[\\/]/)
      .map(encodeSegment)
      .join("/");
    const url = new URL(`/files/${segments}`, base);
    const token = base.searchParams.get("token");
    if (token) url.searchParams.set("token", token);
    return url.toString();
  }

  /** Stops the server, and removes its directory when we created it. */
  async stop(): Promise<void> {
    if (this.process.exitCode === null && this.process.signalCode === null) {
      const exited = this.exited;
      this.process.kill("SIGINT");
      const kill = setTimeout(() => this.process.kill("SIGKILL"), 3_000);
      await exited;
      clearTimeout(kill);
    }
    if (this.owned) rmSync(this.dir, { recursive: true, force: true });
  }
}

/** Splits `stream` into lines; http:// lines also go to `urls`. */
function collect(
  stream: Readable | null,
  output: string[],
  urls: string[] | null,
  announce: () => void,
): void {
  if (!stream) return;
  stream.setEncoding("utf8");
  let buffer = "";
  stream.on("data", (chunk: string) => {
    buffer += chunk;
    for (let at = buffer.indexOf("\n"); at >= 0; at = buffer.indexOf("\n")) {
      const line = buffer.slice(0, at);
      buffer = buffer.slice(at + 1);
      output.push(line);
      if (urls && line.startsWith("http://")) {
        urls.push(line);
        announce();
      }
    }
  });
  stream.on("close", () => {
    if (buffer) output.push(buffer);
  });
}

/** Escapes one url path segment the way the server does. */
function encodeSegment(segment: string): string {
  let out = "";
  for (const byte of Buffer.from(segment, "utf8")) {
    const c = String.fromCharCode(byte);
    out += /[A-Za-z0-9\-._~]/.test(c) ? c : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return out;
}

/** The keys of `value` as TOML, nested objects as dotted tables. */
function toToml(value: Record<string, unknown>, prefix = ""): string {
  const scalars: string[] = [];
  const tables: string[] = [];
  for (const [key, v] of Object.entries(value)) {
    const name = prefix ? `${prefix}.${key}` : key;
    if (v !== null && typeof v === "object" && !Array.isArray(v)) {
      tables.push(`[${name}]`);
      tables.push(toToml(v as Record<string, unknown>, name));
    } else {
      scalars.push(`${key} = ${tomlValue(v)}`);
    }
  }
  return [...scalars, ...tables].join("\n") + "\n";
}

function tomlValue(value: unknown): string {
  return typeof value === "string" ? JSON.stringify(value) : JSON.stringify(value ?? "null");
}
