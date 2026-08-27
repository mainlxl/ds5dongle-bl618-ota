export const REPORT_ID_CONFIG = 0xf6;
export const REPORT_ID_VERSION = 0xf8;
export const REPORT_ID_STATUS = 0xf9;

export const OTA_CMD_START = 0x10;
export const OTA_CMD_DATA = 0x11;
export const OTA_CMD_FINISH = 0x12;
export const OTA_CMD_ABORT = 0x13;

export const OTA_CHUNK_SIZE = 59;

export const OTA_STATUS = {
  0: "idle",
  1: "receiving",
  2: "verifying",
  3: "complete",
  224: "generic error",
  225: "checksum error",
  226: "signature error",
  227: "device error",
  228: "image error",
  255: "error",
};

export const OTA_ERROR_DETAIL = {
  0: "none",
  1: "partition lookup failed",
  2: "active partition table failed",
  3: "FW entry failed",
  4: "partition switch failed",
  5: "bad OTA magic",
  6: "bad OTA type",
  7: "bad OTA size",
  8: "image too large",
  9: "flash erase failed",
  10: "flash write failed",
  11: "flash readback failed",
  12: "flash compare mismatch",
  13: "payload too large",
  14: "bad payload magic",
  15: "SHA init failed",
  16: "SHA update failed",
  17: "SHA finish failed",
  18: "SHA mismatch",
  19: "package too small",
  20: "incomplete package",
  21: "incomplete flash",
  22: "sequence mismatch",
  23: "short command",
};

export function readUint32LE(bytes, offset) {
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

export function writeUint32LE(bytes, offset, value) {
  bytes[offset] = value & 0xff;
  bytes[offset + 1] = (value >>> 8) & 0xff;
  bytes[offset + 2] = (value >>> 16) & 0xff;
  bytes[offset + 3] = (value >>> 24) & 0xff;
}

export function reportPayload(dataView, reportId) {
  const bytes = new Uint8Array(dataView.buffer, dataView.byteOffset, dataView.byteLength);
  if (bytes.length > 0 && bytes[0] === reportId) {
    return bytes.slice(1);
  }
  return bytes;
}

export function parseFirmwareVersion(dataView) {
  const payload = reportPayload(dataView, REPORT_ID_VERSION);
  const end = payload.indexOf(0);
  const versionBytes = end >= 0 ? payload.slice(0, end) : payload;
  return new TextDecoder().decode(versionBytes).trim();
}

export function parseOtaStatus(dataView) {
  const payload = reportPayload(dataView, REPORT_ID_STATUS);
  const status = payload[2] ?? 0;
  const received = payload.length >= 7 ? readUint32LE(payload, 3) : 0;
  const total = payload.length >= 11 ? readUint32LE(payload, 7) : 0;
  const errorDetail = payload.length >= 14 ? payload[13] : 0;
  const errorAddress = payload.length >= 18 ? readUint32LE(payload, 14) : 0;
  const payloadFlushed = payload.length >= 22 ? readUint32LE(payload, 18) : 0;
  return {
    status,
    label: OTA_STATUS[status] || `status ${status}`,
    received,
    total,
    errorDetail,
    errorLabel: OTA_ERROR_DETAIL[errorDetail] || `detail ${errorDetail}`,
    errorAddress,
    payloadFlushed,
    active: status !== 0 || total !== 0,
  };
}

export function inferSpeedFromVersion(version) {
  return /\dH($|[^A-Za-z0-9])/i.test(`${version} `) || /-hs\b/i.test(version)
    ? "hs"
    : "fs";
}

export function parseSha256Sums(text) {
  const sums = new Map();
  for (const line of text.split(/\r?\n/)) {
    const match = line.match(/^([a-f0-9]{64})\s+\*?(.+)$/i);
    if (match) {
      sums.set(match[2].trim(), match[1].toLowerCase());
    }
  }
  return sums;
}

export function bytesToHex(bytes) {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

export async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return bytesToHex(new Uint8Array(digest));
}

export function selectReleaseAsset(release, speed) {
  const tag = release.tag_name || "";
  const safeTag = tag.replaceAll("/", "-");
  const suffix = speed === "hs" ? "-hs" : "";
  const exactName = `ds5dongle-lctech616-${safeTag}${suffix}.bin.ota`;
  const exact = release.assets?.find((asset) => asset.name === exactName);
  if (exact) {
    return exact;
  }

  const pattern = speed === "hs"
    ? /^ds5dongle-lctech616-.+-hs\.bin\.ota$/i
    : /^ds5dongle-lctech616-.+(?<!-hs)\.bin\.ota$/i;
  return release.assets?.find((asset) => pattern.test(asset.name)) || null;
}

export function selectChecksumAsset(release, speed) {
  const tag = (release.tag_name || "").replaceAll("/", "-");
  const exactName = `SHA256SUMS-${tag}-${speed}.txt`;
  return release.assets?.find((asset) => asset.name === exactName) ||
    release.assets?.find((asset) => asset.name.startsWith("SHA256SUMS-") && asset.name.endsWith(`-${speed}.txt`)) ||
    null;
}

export function makeStartCommand(packageSize) {
  const command = new Uint8Array(5);
  command[0] = OTA_CMD_START;
  writeUint32LE(command, 1, packageSize);
  return command;
}

export function makeDataCommand(seq, chunk) {
  if (chunk.length > OTA_CHUNK_SIZE) {
    throw new RangeError(`chunk is ${chunk.length} bytes, max is ${OTA_CHUNK_SIZE}`);
  }
  const command = new Uint8Array(4 + chunk.length);
  command[0] = OTA_CMD_DATA;
  command[1] = seq & 0xff;
  command[2] = (seq >>> 8) & 0xff;
  command[3] = chunk.length;
  command.set(chunk, 4);
  return command;
}

export function makeSimpleCommand(commandId) {
  return new Uint8Array([commandId]);
}

export function createDataCommands(packageBytes, chunkSize = OTA_CHUNK_SIZE) {
  const commands = [];
  let seq = 0;
  for (let offset = 0; offset < packageBytes.length; offset += chunkSize) {
    commands.push(makeDataCommand(seq, packageBytes.slice(offset, offset + chunkSize)));
    seq += 1;
  }
  return commands;
}
