import assert from "node:assert/strict";
import test from "node:test";
import {
  OTA_CHUNK_SIZE,
  OTA_CMD_DATA,
  OTA_CMD_START,
  REPORT_ID_STATUS,
  REPORT_ID_VERSION,
  createDataCommands,
  inferSpeedFromVersion,
  makeStartCommand,
  parseFirmwareVersion,
  parseOtaStatus,
  parseSha256Sums,
  selectReleaseAsset,
} from "../ota-core.js";

test("parses firmware version reports with or without report id", () => {
  const withId = new DataView(Uint8Array.from([
    REPORT_ID_VERSION,
    ..."LCT616-DS5 3.18H".split("").map((char) => char.charCodeAt(0)),
    0,
  ]).buffer);
  const withoutId = new DataView(Uint8Array.from([
    ..."LCT616-DS5 3.18".split("").map((char) => char.charCodeAt(0)),
    0,
  ]).buffer);

  assert.equal(parseFirmwareVersion(withId), "LCT616-DS5 3.18H");
  assert.equal(parseFirmwareVersion(withoutId), "LCT616-DS5 3.18");
});

test("parses OTA progress reports with or without report id", () => {
  const withId = Uint8Array.from([
    REPORT_ID_STATUS,
    0,
    0x80,
    1,
    0x34,
    0x12,
    0,
    0,
    0x78,
    0x56,
    0,
    0,
    50,
    0,
    12,
    0xcd,
    0xab,
    0x21,
    0,
    0x00,
    0xc0,
    0x0b,
    0,
  ]);
  const withoutId = withId.slice(1);

  assert.deepEqual(parseOtaStatus(new DataView(withId.buffer)), {
    status: 1,
    label: "receiving",
    received: 0x1234,
    total: 0x5678,
    errorDetail: 12,
    errorLabel: "flash compare mismatch",
    errorAddress: 0x21abcd,
    payloadFlushed: 0x0bc000,
    active: true,
  });
  assert.deepEqual(parseOtaStatus(new DataView(withoutId.buffer)), {
    status: 1,
    label: "receiving",
    received: 0x1234,
    total: 0x5678,
    errorDetail: 12,
    errorLabel: "flash compare mismatch",
    errorAddress: 0x21abcd,
    payloadFlushed: 0x0bc000,
    active: true,
  });
});

test("selects FS and HS release OTA assets", () => {
  const release = {
    tag_name: "v3.18",
    assets: [
      { name: "ds5dongle-lctech616-v3.18.bin.ota" },
      { name: "ds5dongle-lctech616-v3.18-hs.bin.ota" },
      { name: "ds5dongle-lctech616-v3.18.xz.ota" },
    ],
  };

  assert.equal(selectReleaseAsset(release, "fs").name, "ds5dongle-lctech616-v3.18.bin.ota");
  assert.equal(selectReleaseAsset(release, "hs").name, "ds5dongle-lctech616-v3.18-hs.bin.ota");
});

test("creates start and sequenced data commands", () => {
  const start = makeStartCommand(0x12345678);
  assert.equal(start[0], OTA_CMD_START);
  assert.deepEqual([...start.slice(1)], [0x78, 0x56, 0x34, 0x12]);

  const packageBytes = Uint8Array.from({ length: OTA_CHUNK_SIZE + 3 }, (_, index) => index & 0xff);
  const commands = createDataCommands(packageBytes);
  assert.equal(commands.length, 2);
  assert.equal(commands[0][0], OTA_CMD_DATA);
  assert.equal(commands[0][1], 0);
  assert.equal(commands[0][3], OTA_CHUNK_SIZE);
  assert.equal(commands[1][1], 1);
  assert.equal(commands[1][3], 3);
});

test("parses checksum files and infers speed", () => {
  const sums = parseSha256Sums("a".repeat(64) + "  ds5dongle.bin.ota\n");
  assert.equal(sums.get("ds5dongle.bin.ota"), "a".repeat(64));
  assert.equal(inferSpeedFromVersion("LCT616-DS5 3.18H"), "hs");
  assert.equal(inferSpeedFromVersion("LCT616-DS5 3.18"), "fs");
});
