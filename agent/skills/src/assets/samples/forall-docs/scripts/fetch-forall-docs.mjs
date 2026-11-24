#!/usr/bin/env node
import {
  access,
  mkdir,
  readFile,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import path from "node:path";
import process from "node:process";
import { promisify } from "node:util";

const GITHUB_RAW_BASE =
  "https://raw.githubusercontent.com/astrio-labs/forall/main/";
const DOC_PATHS = [
  "README.md",
  "docs/getting-started.md",
  "docs/architecture.md",
  "docs/hosted-mcp.md",
  "docs/workflow.md",
  "docs/project-layout.md",
];
const DEFAULT_CACHE_DIR_NAME = "forall-docs-cache";
const DOCS_FILE_NAME = "forall-docs.md";
const OUTLINE_FILE_NAME = "forall-docs.outline.md";
const USER_AGENT = "forall-docs-skill";
const execFileAsync = promisify(execFile);

class DocsFetchError extends Error {
  constructor(message, options) {
    super(message, options);
    this.name = "DocsFetchError";
  }
}

const sha256 = (value) => createHash("sha256").update(value).digest("hex");

const proxyConfigured = () =>
  Boolean(
    process.env.HTTP_PROXY ||
      process.env.HTTPS_PROXY ||
      process.env.http_proxy ||
      process.env.https_proxy,
  );

const requestText = async (url, { cacheDir, timeoutMs }) => {
  const curlNames =
    process.platform === "win32" ? ["curl.exe", "curl"] : ["curl"];
  const headerPath = path.join(
    cacheDir,
    `.headers-${process.pid}-${Date.now()}`,
  );
  const bodyPath = path.join(cacheDir, `.body-${process.pid}-${Date.now()}`);
  const args = [
    "--silent",
    "--show-error",
    "--location",
    "--fail",
    "--user-agent",
    USER_AGENT,
    "--max-time",
    String(Math.max(1, Math.ceil(timeoutMs / 1000))),
    "--output",
    bodyPath,
    url,
  ];

  if (typeof fetch === "function" && !proxyConfigured()) {
    const response = await fetch(url, {
      headers: { "User-Agent": USER_AGENT },
    });
    if (!response.ok) {
      throw new DocsFetchError(`GET ${url} failed with ${response.status}`);
    }
    return response.text();
  }

  let lastError;
  for (const curlName of curlNames) {
    try {
      await execFileAsync(curlName, args, { windowsHide: true });
      const body = await readFile(bodyPath, "utf8");
      return body;
    } catch (error) {
      lastError = error;
      if (error?.code !== "ENOENT") break;
    } finally {
      await Promise.all([
        rm(headerPath, { force: true }),
        rm(bodyPath, { force: true }),
      ]);
    }
  }

  throw new DocsFetchError(`Could not fetch ${url}`, { cause: lastError });
};

const usableCacheDir = async (candidate) => {
  const resolved = path.resolve(candidate);
  await mkdir(resolved, { recursive: true });
  try {
    await access(resolved, fsConstants.W_OK | fsConstants.X_OK);
  } catch {
    return null;
  }
  return resolved;
};

const resolveCacheDir = async (cacheDir) => {
  if (cacheDir) {
    return usableCacheDir(cacheDir);
  }
  for (const base of [
    process.env.TMPDIR,
    process.env.TEMP,
    process.env.TMP,
    process.platform !== "win32" ? "/tmp" : undefined,
  ]) {
    if (!base) continue;
    const usable = await usableCacheDir(
      path.join(base, DEFAULT_CACHE_DIR_NAME),
    );
    if (usable) return usable;
  }
  return null;
};

const buildOutline = (manual) => {
  const lines = manual.replace(/\r\n/g, "\n").split("\n");
  if (lines.at(-1) === "") lines.pop();
  const headings = [];
  let inFence = false;

  lines.forEach((line, index) => {
    if (/^\s*(```|~~~)/.test(line)) {
      inFence = !inFence;
      return;
    }
    if (inFence) return;
    const match = /^(#{1,6})\s+(.+?)\s*$/.exec(line);
    if (!match) return;
    const level = match[1].length;
    if (level < 2 || level > 3) return;
    headings.push({
      level,
      title: match[2].replace(/\s+#+\s*$/, "").trim(),
      startLine: index + 1,
      endLine: lines.length,
    });
  });

  for (let index = 0; index < headings.length; index += 1) {
    const heading = headings[index];
    const nextPeer = headings
      .slice(index + 1)
      .find((candidate) => candidate.level <= heading.level);
    if (nextPeer) heading.endLine = nextPeer.startLine - 1;
  }

  const body =
    headings.length === 0
      ? "_No headings found._\n"
      : headings
          .map(
            (heading) =>
              `- ${"#".repeat(heading.level)} ${heading.title} (lines ${heading.startLine}-${heading.endLine})`,
          )
          .join("\n") + "\n";

  return { body, headingCount: headings.length, lineCount: lines.length };
};

const fetchForallDocs = async ({ cacheDir, timeoutMs = 30000, repoBase }) => {
  const resolvedCacheDir = await resolveCacheDir(cacheDir);
  if (!resolvedCacheDir) {
    throw new DocsFetchError(
      "Cache directory unavailable; pass --cache-dir or use references/product.md.",
    );
  }

  const sections = [];
  for (const docPath of DOC_PATHS) {
    const url = new URL(docPath, repoBase).toString();
    const text = await requestText(url, {
      cacheDir: resolvedCacheDir,
      timeoutMs,
    });
    sections.push(`<!-- source: ${docPath} -->\n\n${text.trim()}\n`);
  }

  const combined = `# Forall public documentation (cached)\n\n${sections.join("\n---\n\n")}\n`;
  const digest = sha256(combined);
  const docsPath = path.join(resolvedCacheDir, DOCS_FILE_NAME);
  const outlinePath = path.join(resolvedCacheDir, OUTLINE_FILE_NAME);
  const metaPath = path.join(resolvedCacheDir, ".forall-docs.sha256");

  let cacheStatus = "updated";
  try {
    const previous = await readFile(metaPath, "utf8");
    if (previous.trim() === digest) cacheStatus = "hit";
  } catch {
    // no cache yet
  }

  const tmpDocs = path.join(resolvedCacheDir, `.${DOCS_FILE_NAME}.tmp`);
  await writeFile(tmpDocs, combined, "utf8");
  await rename(tmpDocs, docsPath);
  await writeFile(metaPath, `${digest}\n`, "utf8");

  const outline = buildOutline(combined);
  const outlineText = `# Forall docs outline\n\n${outline.body}`;
  const tmpOutline = path.join(resolvedCacheDir, `.${OUTLINE_FILE_NAME}.tmp`);
  await writeFile(tmpOutline, outlineText, "utf8");
  await rename(tmpOutline, outlinePath);

  return {
    outlineText,
    status: {
      repoBase,
      docPaths: DOC_PATHS,
      sha256: digest,
      cacheStatus,
      cacheDir: resolvedCacheDir,
      docsPath,
      outlinePath,
      checkedAt: new Date().toISOString(),
      lineCount: outline.lineCount,
      headingCount: outline.headingCount,
    },
  };
};

const parseArgs = (argv) => {
  const args = {
    repoBase: GITHUB_RAW_BASE,
    cacheDir: undefined,
    timeoutMs: 30000,
    statusJson: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--repo-base") args.repoBase = argv[++index];
    else if (arg === "--cache-dir") args.cacheDir = argv[++index];
    else if (arg === "--timeout-ms") args.timeoutMs = Number(argv[++index]);
    else if (arg === "--status-json") args.statusJson = true;
    else throw new DocsFetchError(`Unknown argument: ${arg}`);
  }
  if (!args.repoBase.endsWith("/")) args.repoBase += "/";
  return args;
};

const formatResult = ({ status, outlineText }) =>
  [
    "Forall public docs cached.",
    `docs: ${status.docsPath}`,
    `outline: ${status.outlinePath}`,
    `status: ${status.cacheStatus}`,
    `sha256: ${status.sha256}`,
    "",
    outlineText,
  ].join("\n");

const main = async () => {
  const args = parseArgs(process.argv.slice(2));
  const result = await fetchForallDocs(args);
  process.stdout.write(formatResult(result));
  if (args.statusJson) {
    console.error(JSON.stringify(result.status));
  }
};

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
});

export { fetchForallDocs, buildOutline };
