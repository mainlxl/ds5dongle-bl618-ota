const DEFAULT_REPO = "mainlxl/ds5dongle-bl618-ota";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    try {
      if (url.pathname === "/api/latest") {
        return handleLatest(url, env);
      }
      if (url.pathname === "/api/download") {
        return handleDownload(url, env);
      }
      return env.ASSETS.fetch(request);
    } catch (error) {
      return text(error.message || String(error), 400);
    }
  },
};

async function handleLatest(url) {
  const repo = normalizeRepo(url.searchParams.get("repo") || DEFAULT_REPO);
  const speed = normalizeSpeed(url.searchParams.get("speed") || "hs");
  const latestUrl = `https://github.com/${repo}/releases/latest`;
  const latest = await fetch(latestUrl, {
    redirect: "manual",
    headers: { "User-Agent": "ds5dongle-web-ota/0.1" },
  });

  let location = latest.headers.get("location") || "";
  if (!location && latest.url) {
    location = latest.url;
  }
  const tag = extractTag(location);
  const safeTag = tag.replaceAll("/", "-");
  const suffix = speed === "hs" ? "-hs" : "";
  const otaName = `ds5dongle-lctech616-${safeTag}${suffix}.bin.ota`;
  const checksumName = `SHA256SUMS-${safeTag}-${speed}.txt`;

  return json({
    repo,
    tag_name: tag,
    name: tag,
    html_url: `https://github.com/${repo}/releases/tag/${tag}`,
    assets: [
      {
        name: otaName,
        browser_download_url: `/api/download?repo=${encodeURIComponent(repo)}&tag=${encodeURIComponent(tag)}&asset=${encodeURIComponent(otaName)}`,
      },
      {
        name: checksumName,
        browser_download_url: `/api/download?repo=${encodeURIComponent(repo)}&tag=${encodeURIComponent(tag)}&asset=${encodeURIComponent(checksumName)}`,
      },
    ],
  });
}

async function handleDownload(url) {
  const repo = normalizeRepo(required(url, "repo"));
  const tag = required(url, "tag");
  const asset = required(url, "asset");
  validateTag(tag);
  validateAsset(asset);

  const assetUrl = `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(asset)}`;
  const upstream = await fetch(assetUrl, {
    redirect: "follow",
    headers: { "User-Agent": "ds5dongle-web-ota/0.1" },
  });
  if (!upstream.ok) {
    return text(`GitHub Release 附件下载失败：HTTP ${upstream.status}`, upstream.status);
  }

  const headers = new Headers();
  headers.set("Cache-Control", "public, max-age=300");
  headers.set("Content-Disposition", `attachment; filename="${asset.replaceAll('"', "")}"`);
  headers.set("Content-Type", asset.endsWith(".txt") ? "text/plain; charset=utf-8" : "application/octet-stream");
  const length = upstream.headers.get("content-length");
  if (length) {
    headers.set("Content-Length", length);
  }

  return new Response(upstream.body, { status: 200, headers });
}

function required(url, name) {
  const value = url.searchParams.get(name);
  if (!value) {
    throw new Error(`缺少参数：${name}`);
  }
  return value;
}

function normalizeRepo(value) {
  const repo = String(value)
    .trim()
    .replace(/^https:\/\/github\.com\//, "")
    .replace(/\/$/, "")
    .replace(/\.git$/, "");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) {
    throw new Error("仓库格式应为 owner/name");
  }
  return repo;
}

function normalizeSpeed(value) {
  const speed = String(value).toLowerCase();
  if (speed !== "fs" && speed !== "hs") {
    throw new Error("固件类型只能是 fs 或 hs");
  }
  return speed;
}

function extractTag(location) {
  const match = String(location).match(/\/releases\/tag\/([^/?#]+)/);
  if (!match) {
    throw new Error("无法解析 GitHub latest release tag");
  }
  return decodeURIComponent(match[1]);
}

function validateTag(tag) {
  if (!/^[A-Za-z0-9._/-]+$/.test(tag) || tag.includes("..")) {
    throw new Error("tag 格式不正确");
  }
}

function validateAsset(asset) {
  if (
    !/^ds5dongle-lctech616-[A-Za-z0-9._-]+(-hs)?\.bin\.ota$/.test(asset) &&
    !/^SHA256SUMS-[A-Za-z0-9._-]+-(fs|hs)\.txt$/.test(asset)
  ) {
    throw new Error("附件名不在允许列表");
  }
}

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
    },
  });
}

function text(data, status = 200) {
  return new Response(data, {
    status,
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
}
