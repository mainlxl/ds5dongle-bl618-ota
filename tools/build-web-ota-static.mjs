import { createHash } from "node:crypto";
import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import https from "node:https";
import path from "node:path";

const sourceDir = process.env.WEB_OTA_SOURCE_DIR || "web-ota";
const outputDir = process.env.WEB_OTA_OUTPUT_DIR || "web-ota-static";
const assetsDir = process.env.RELEASE_ASSETS_DIR || "release-assets";
const siteUrl = trimSlash(process.env.WEB_OTA_SITE_URL || "https://ds5dongle.xdfg.cc");
const repo = process.env.GITHUB_REPOSITORY || "mainlxl/ds5dongle-bl618-ota";
const releaseTag = process.env.RELEASE_TAG || "";
const keepReleases = Number.parseInt(process.env.WEB_OTA_KEEP_RELEASES || "100", 10);

if (!Number.isFinite(keepReleases) || keepReleases < 1) {
  throw new Error("WEB_OTA_KEEP_RELEASES must be a positive number");
}

await rm(outputDir, { recursive: true, force: true });
await mkdir(outputDir, { recursive: true });
await copyStaticShell(sourceDir, outputDir);

const previous = await fetchPreviousManifest(siteUrl);
const previousReleases = previous.releases || [];
const current = releaseTag ? await collectCurrentRelease(assetsDir, releaseTag) : null;
if (!current && previousReleases.length === 0) {
  throw new Error("No RELEASE_TAG provided and no previous manifest exists");
}
const releases = [
  ...(current ? [current] : []),
  ...previousReleases.filter((item) => item.tag_name !== releaseTag),
].slice(0, keepReleases);

for (const release of releases) {
  if (release.tag_name === releaseTag) {
    continue;
  }
  await restorePreviousRelease(siteUrl, outputDir, release);
}

const manifest = {
  schema: 1,
  repo,
  generated_at: new Date().toISOString(),
  latest: releases[0]?.tag_name || releaseTag,
  keep_releases: keepReleases,
  releases,
};
await writeFile(
  path.join(outputDir, "manifest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);

console.log(`Built static Web OTA site: ${outputDir}`);
console.log(`Latest: ${manifest.latest}`);
console.log(`Versions kept: ${releases.length}`);

async function copyStaticShell(from, to) {
  await mkdir(to, { recursive: true });
  const entries = await readdir(from, { withFileTypes: true });
  for (const entry of entries) {
    if (["_worker.js", "functions", "tests", "node_modules"].includes(entry.name)) {
      continue;
    }
    if (entry.name === "package.json" || entry.name === "README.md") {
      continue;
    }
    if (entry.name === "releases") {
      continue;
    }

    const src = path.join(from, entry.name);
    const dst = path.join(to, entry.name);
    if (entry.isDirectory()) {
      await copyStaticShell(src, dst);
    } else if (entry.isFile()) {
      await mkdir(path.dirname(dst), { recursive: true });
      await writeFile(dst, await readFile(src));
    }
  }
}

async function fetchPreviousManifest(baseUrl) {
  try {
    const response = await request(`${baseUrl}/manifest.json`);
    if (response.statusCode < 200 || response.statusCode >= 300) {
      return { releases: [] };
    }
    return JSON.parse(response.body.toString("utf8"));
  } catch (error) {
    console.warn(`No previous manifest loaded: ${error.message}`);
    return { releases: [] };
  }
}

async function collectCurrentRelease(dir, tag) {
  const files = await listFiles(dir);
  const wanted = files.filter((file) => {
    const name = path.basename(file);
    return name.endsWith(".bin.ota") || /^SHA256SUMS-.+-(fs|hs)\.txt$/.test(name);
  });
  if (wanted.length === 0) {
    throw new Error(`No OTA assets found in ${dir}`);
  }

  const releaseDir = path.join(outputDir, "releases", safePathSegment(tag));
  await mkdir(releaseDir, { recursive: true });

  const assets = [];
  for (const file of wanted.sort()) {
    const name = path.basename(file);
    const bytes = await readFile(file);
    await writeFile(path.join(releaseDir, name), bytes);
    assets.push({
      name,
      size: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      browser_download_url: `/releases/${safePathSegment(tag)}/${name}`,
    });
  }

  return {
    tag_name: tag,
    name: tag,
    created_at: new Date().toISOString(),
    assets,
  };
}

async function restorePreviousRelease(baseUrl, root, release) {
  const releaseDir = path.join(root, "releases", safePathSegment(release.tag_name));
  await mkdir(releaseDir, { recursive: true });
  const restored = [];

  for (const asset of release.assets || []) {
    if (!asset.browser_download_url || !asset.name) {
      continue;
    }
    const relativeUrl = asset.browser_download_url.startsWith("/")
      ? asset.browser_download_url
      : new URL(asset.browser_download_url).pathname;
    const source = `${baseUrl}${relativeUrl}`;
    const target = path.join(releaseDir, asset.name);
    try {
      await download(source, target);
      restored.push({
        ...asset,
        browser_download_url: `/releases/${safePathSegment(release.tag_name)}/${asset.name}`,
      });
    } catch (error) {
      console.warn(`Skip old asset ${release.tag_name}/${asset.name}: ${error.message}`);
    }
  }

  release.assets = restored;
}

async function download(url, target) {
  const response = await request(url);
  if (response.statusCode < 200 || response.statusCode >= 300) {
    throw new Error(`HTTP ${response.statusCode}`);
  }
  await writeFile(target, response.body);
}

async function listFiles(dir) {
  const result = [];
  const entries = await readdir(dir, { withFileTypes: true });
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      result.push(...await listFiles(full));
    } else if (entry.isFile()) {
      result.push(full);
    }
  }
  return result;
}

function safePathSegment(value) {
  return String(value).replaceAll("/", "-");
}

function trimSlash(value) {
  return String(value).replace(/\/+$/, "");
}

function request(url, redirects = 5) {
  return new Promise((resolve, reject) => {
    const parsed = new URL(url);
    const client = parsed.protocol === "http:" ? http : https;
    const req = client.get(
      parsed,
      {
        headers: {
          "User-Agent": "ds5dongle-web-ota-static/0.1",
        },
      },
      (res) => {
        const statusCode = res.statusCode || 0;
        const location = res.headers.location;
        if ([301, 302, 303, 307, 308].includes(statusCode) && location) {
          res.resume();
          if (redirects <= 0) {
            reject(new Error("too many redirects"));
            return;
          }
          resolve(request(new URL(location, parsed).href, redirects - 1));
          return;
        }

        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => {
          resolve({
            statusCode,
            headers: res.headers,
            body: Buffer.concat(chunks),
          });
        });
      },
    );
    req.on("error", reject);
    req.setTimeout(30_000, () => {
      req.destroy(new Error("request timeout"));
    });
  });
}
