#include "ota_update.h"
#include "bflb_flash.h"
#include "bflb_l1c.h"
#include "bflb_mtimer.h"
#include "debug_log.h"
#include "partition.h"
#include <string.h>
#include <tinycrypt/constants.h>
#include <tinycrypt/sha256.h>

#define OTA_CMD_START  0x10
#define OTA_CMD_DATA   0x11
#define OTA_CMD_FINISH 0x12
#define OTA_CMD_ABORT  0x13

#define OTA_HEADER_SIZE     512U
#define OTA_MAGIC           "BL60X_OTA_Ver1.0"
#define OTA_MAGIC_LEN       16U
#define OTA_SECTOR_SIZE     4096U
#define OTA_WRITE_BUF_SIZE  4096U
#define OTA_READ_BUF_SIZE   256U
#define OTA_REBOOT_DELAY_US 800000ULL

struct ota_update_ctx {
    uint8_t status;
    uint8_t header[OTA_HEADER_SIZE];
    uint8_t write_buf[OTA_WRITE_BUF_SIZE];
    uint8_t expected_sha256[32];
    uint32_t package_size;
    uint32_t package_received;
    uint32_t payload_size;
    uint32_t payload_received;
    uint32_t payload_flushed;
    uint32_t write_buf_len;
    uint32_t flash_addr;
    uint32_t flash_max_len;
    uint32_t erased_until;
    uint16_t expected_seq;
    uint8_t inactive_index;
    bool active;
    bool header_ready;
    bool reboot_pending;
    uint64_t reboot_at_us;
    pt_table_id_type target_table_id;
};

static struct ota_update_ctx ota;
static struct tc_sha256_state_struct sha_ctx;
static __attribute__((aligned(32))) uint8_t readback_buf[OTA_READ_BUF_SIZE];

static uint32_t read_le32(const uint8_t *p)
{
    return ((uint32_t)p[0]) |
           ((uint32_t)p[1] << 8) |
           ((uint32_t)p[2] << 16) |
           ((uint32_t)p[3] << 24);
}

static void write_le32(uint8_t *p, uint32_t v)
{
    p[0] = v & 0xFF;
    p[1] = (v >> 8) & 0xFF;
    p[2] = (v >> 16) & 0xFF;
    p[3] = (v >> 24) & 0xFF;
}

static void ota_set_error(uint8_t status, uint8_t detail, uint32_t address,
                          const char *msg)
{
    ota.status = status;
    ota.active = false;
    ota.reboot_pending = false;
    LOG_ERR("[OTA] %s (status=%u detail=%u addr=0x%08lx)\n",
            msg,
            status,
            detail,
            (unsigned long)address);
}

static int partition_flash_erase(uint32_t addr, uint32_t len)
{
    return bflb_flash_erase(addr, len);
}

static int partition_flash_write(uint32_t addr, uint8_t *data, uint32_t len)
{
    return bflb_flash_write(addr, data, len);
}

static int partition_flash_read(uint32_t addr, uint8_t *data, uint32_t len)
{
    return bflb_flash_read(addr, data, len);
}

static bool ota_prepare_partition(void)
{
    pt_table_stuff_config pt[2];
    pt_table_entry_config entry;
    pt_table_id_type active_id;

    pt_table_set_flash_operation(partition_flash_erase,
                                 partition_flash_write,
                                 partition_flash_read);

    active_id = pt_table_get_active_partition_need_lock(pt);
    if (active_id == PT_TABLE_ID_INVALID)
        return false;

    if (pt_table_get_active_entries_by_id(&pt[active_id], PT_ENTRY_FW_CPU0,
                                          &entry) != PT_ERROR_SUCCESS)
        return false;

    ota.inactive_index = !(entry.active_index & 0x01);
    ota.target_table_id = active_id == PT_TABLE_ID_0 ? PT_TABLE_ID_1 : PT_TABLE_ID_0;
    ota.flash_addr = entry.start_address[ota.inactive_index & 1];
    ota.flash_max_len = entry.max_len[ota.inactive_index & 1];
    ota.erased_until = ota.flash_addr;
    return ota.flash_addr != 0 && ota.flash_max_len != 0;
}

static bool ota_switch_partition(void)
{
    pt_table_stuff_config pt[2];
    pt_table_entry_config entry;
    pt_table_id_type active_id;

    active_id = pt_table_get_active_partition_need_lock(pt);
    if (active_id == PT_TABLE_ID_INVALID) {
        ota_set_error(OTA_STATUS_ERROR, OTA_ERR_ACTIVE_TABLE, 0,
                      "active partition table not found");
        return false;
    }

    if (pt_table_get_active_entries_by_id(&pt[active_id], PT_ENTRY_FW_CPU0,
                                          &entry) != PT_ERROR_SUCCESS) {
        ota_set_error(OTA_STATUS_ERROR, OTA_ERR_FW_ENTRY, 0,
                      "FW partition entry not found");
        return false;
    }

    entry.active_index = ota.inactive_index & 0x01;
    entry.len = ota.payload_size;
    entry.age++;

    if (pt_table_update_entry(ota.target_table_id, &pt[active_id], &entry) !=
        PT_ERROR_SUCCESS) {
        ota_set_error(OTA_STATUS_ERROR, OTA_ERR_PARTITION_SWITCH, 0,
                      "partition switch failed");
        return false;
    }

    return true;
}

static bool ota_parse_header(void)
{
    if (memcmp(ota.header, OTA_MAGIC, OTA_MAGIC_LEN) != 0) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_BAD_MAGIC, 0,
                      "bad OTA magic");
        return false;
    }

    if (memcmp(ota.header + 16, "RAW", 3) != 0) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_BAD_TYPE, 0,
                      "only RAW OTA packages are accepted");
        return false;
    }

    ota.payload_size = read_le32(ota.header + 20);
    memcpy(ota.expected_sha256, ota.header + 64, sizeof(ota.expected_sha256));

    if (ota.payload_size == 0 ||
        ota.package_size != OTA_HEADER_SIZE + ota.payload_size) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_BAD_SIZE, 0,
                      "bad OTA size");
        return false;
    }

    if (ota.payload_size > ota.flash_max_len) {
        ota_set_error(OTA_STATUS_ERR_DEVICE, OTA_ERR_IMAGE_TOO_LARGE, 0,
                      "OTA image is too large");
        return false;
    }

    ota.header_ready = true;
    LOG_INF("[OTA] Header OK: payload=%lu target=0x%08lx/%lu\n",
            (unsigned long)ota.payload_size,
            (unsigned long)ota.flash_addr,
            (unsigned long)ota.flash_max_len);
    return true;
}

static bool ota_erase_for_write(uint32_t addr, uint32_t len)
{
    uint32_t end = addr + len;
    uint32_t erase_end = (end + OTA_SECTOR_SIZE - 1) & ~(OTA_SECTOR_SIZE - 1);

    while (ota.erased_until < erase_end) {
        if (bflb_flash_erase(ota.erased_until, OTA_SECTOR_SIZE) != 0) {
            ota_set_error(OTA_STATUS_ERROR, OTA_ERR_FLASH_ERASE,
                          ota.erased_until, "flash erase failed");
            return false;
        }
        ota.erased_until += OTA_SECTOR_SIZE;
    }

    return true;
}

static void ota_log_hash(const char *label, const uint8_t hash[32])
{
    LOG_ERR("[OTA] %s "
            "%02x%02x%02x%02x%02x%02x%02x%02x"
            "%02x%02x%02x%02x%02x%02x%02x%02x"
            "%02x%02x%02x%02x%02x%02x%02x%02x"
            "%02x%02x%02x%02x%02x%02x%02x%02x\n",
            label,
            hash[0], hash[1], hash[2], hash[3],
            hash[4], hash[5], hash[6], hash[7],
            hash[8], hash[9], hash[10], hash[11],
            hash[12], hash[13], hash[14], hash[15],
            hash[16], hash[17], hash[18], hash[19],
            hash[20], hash[21], hash[22], hash[23],
            hash[24], hash[25], hash[26], hash[27],
            hash[28], hash[29], hash[30], hash[31]);
}

static bool ota_write_flash_chunk(const uint8_t *data, uint32_t len)
{
    uint32_t addr = ota.flash_addr + ota.payload_flushed;
    uint32_t checked = 0;

    if (len == 0)
        return true;

    if (!ota_erase_for_write(addr, len))
        return false;

    if (bflb_flash_write(addr, (uint8_t *)data, len) != 0) {
        ota_set_error(OTA_STATUS_ERROR, OTA_ERR_FLASH_WRITE, addr,
                      "flash write failed");
        return false;
    }

    while (checked < len) {
        uint32_t chunk = len - checked;
        if (chunk > sizeof(readback_buf))
            chunk = sizeof(readback_buf);

        bflb_l1c_dcache_invalidate_range(readback_buf, chunk);
        if (bflb_flash_read(addr + checked, readback_buf, chunk) != 0) {
            ota_set_error(OTA_STATUS_ERROR, OTA_ERR_FLASH_READBACK,
                          addr + checked, "flash readback failed");
            return false;
        }

        if (memcmp(readback_buf, data + checked, chunk) != 0) {
            ota_set_error(OTA_STATUS_ERR_CRC, OTA_ERR_FLASH_COMPARE,
                          addr + checked, "flash write readback mismatch");
            return false;
        }
        checked += chunk;
    }

    ota.payload_flushed += len;
    LOG_DBG("[OTA] Flushed %lu bytes to 0x%08lx\n",
            (unsigned long)len,
            (unsigned long)addr);
    return true;
}

static bool ota_flush_write_buffer(void)
{
    uint32_t len = ota.write_buf_len;

    if (len == 0)
        return true;

    if (!ota_write_flash_chunk(ota.write_buf, len))
        return false;

    ota.write_buf_len = 0;
    return true;
}

static bool ota_write_image_data(const uint8_t *data, uint32_t len)
{
    if (ota.payload_received + len > ota.payload_size) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_TOO_MUCH_PAYLOAD, 0,
                      "too much OTA payload");
        return false;
    }

    if (ota.payload_received == 0 && len >= 4 && memcmp(data, "BFNP", 4) != 0) {
        ota_set_error(OTA_STATUS_ERR_DECRYPT, OTA_ERR_PAYLOAD_MAGIC, 0,
                      "OTA payload is not a bootable BFNP image");
        return false;
    }

    while (len > 0) {
        uint32_t room = OTA_WRITE_BUF_SIZE - ota.write_buf_len;
        uint32_t copied = len < room ? len : room;

        memcpy(ota.write_buf + ota.write_buf_len, data, copied);
        ota.write_buf_len += copied;
        ota.payload_received += copied;
        data += copied;
        len -= copied;

        if (ota.write_buf_len == OTA_WRITE_BUF_SIZE && !ota_flush_write_buffer())
            return false;
    }

    return true;
}

static bool ota_verify_flash(void)
{
    __attribute__((aligned(32))) uint8_t buf[256];
    __attribute__((aligned(32))) uint8_t result[32];
    uint32_t offset = 0;

    if (tc_sha256_init(&sha_ctx) != TC_CRYPTO_SUCCESS) {
        ota_set_error(OTA_STATUS_ERR_DEVICE, OTA_ERR_SHA_INIT, 0,
                      "SHA init failed");
        return false;
    }

    while (offset < ota.payload_size) {
        uint32_t chunk = ota.payload_size - offset;
        if (chunk > sizeof(buf))
            chunk = sizeof(buf);

        bflb_l1c_dcache_invalidate_range(buf, chunk);
        if (bflb_flash_read(ota.flash_addr + offset, buf, chunk) != 0) {
            ota_set_error(OTA_STATUS_ERROR, OTA_ERR_FLASH_READBACK,
                          ota.flash_addr + offset, "flash readback failed");
            return false;
        }

        if (tc_sha256_update(&sha_ctx, buf, chunk) != TC_CRYPTO_SUCCESS) {
            ota_set_error(OTA_STATUS_ERR_CRC, OTA_ERR_SHA_UPDATE,
                          ota.flash_addr + offset, "SHA update failed");
            return false;
        }

        offset += chunk;
    }

    if (tc_sha256_final(result, &sha_ctx) != TC_CRYPTO_SUCCESS) {
        ota_set_error(OTA_STATUS_ERR_CRC, OTA_ERR_SHA_FINISH, 0,
                      "SHA finish failed");
        return false;
    }

    if (memcmp(result, ota.expected_sha256, sizeof(result)) != 0) {
        ota_log_hash("expected SHA256:", ota.expected_sha256);
        ota_log_hash("actual SHA256:  ", result);
        ota_set_error(OTA_STATUS_ERR_CRC, OTA_ERR_SHA_MISMATCH, ota.flash_addr,
                      "flash SHA256 mismatch");
        return false;
    }

    return true;
}

void ota_update_init(void)
{
    memset(&ota, 0, sizeof(ota));
    ota.status = OTA_STATUS_IDLE;
}

static void ota_start(uint32_t package_size)
{
    memset(&ota, 0, sizeof(ota));
    ota.status = OTA_STATUS_RECEIVING;
    ota.package_size = package_size;
    ota.active = true;

    if (package_size < OTA_HEADER_SIZE) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_PACKAGE_TOO_SMALL, 0,
                      "OTA package too small");
        return;
    }

    if (!ota_prepare_partition()) {
        ota_set_error(OTA_STATUS_ERR_DEVICE, OTA_ERR_PARTITION, 0,
                      "could not locate inactive FW slot");
        return;
    }

    LOG_INF("[OTA] Start: package=%lu bytes\n", (unsigned long)package_size);
}

static void ota_data(uint16_t seq, const uint8_t *data, uint32_t len)
{
    uint32_t old_received;
    uint32_t copied = 0;

    if (!ota.active || ota.status != OTA_STATUS_RECEIVING)
        return;

    if (seq != ota.expected_seq) {
        ota_set_error(OTA_STATUS_ERROR, OTA_ERR_SEQUENCE, ota.expected_seq,
                      "unexpected OTA sequence");
        return;
    }
    ota.expected_seq++;

    if (ota.package_received + len > ota.package_size) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_TOO_MUCH_PAYLOAD, 0,
                      "too much OTA data");
        return;
    }

    old_received = ota.package_received;
    if (old_received < OTA_HEADER_SIZE) {
        copied = OTA_HEADER_SIZE - old_received;
        if (copied > len)
            copied = len;
        memcpy(ota.header + old_received, data, copied);
        ota.package_received += copied;
        data += copied;
        len -= copied;

        if (ota.package_received == OTA_HEADER_SIZE && !ota_parse_header())
            return;
    }

    if (len > 0) {
        if (!ota.header_ready) {
            ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_BAD_SIZE, 0,
                          "OTA payload before header");
            return;
        }
        if (!ota_write_image_data(data, len))
            return;
        ota.package_received += len;
    }
}

static void ota_finish(void)
{
    if (!ota.active || ota.status != OTA_STATUS_RECEIVING)
        return;

    ota.status = OTA_STATUS_VERIFYING;

    if (!ota.header_ready || ota.package_received != ota.package_size ||
        ota.payload_received != ota.payload_size) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_INCOMPLETE_PACKAGE, 0,
                      "incomplete OTA package");
        return;
    }

    if (!ota_flush_write_buffer())
        return;

    if (ota.payload_flushed != ota.payload_size) {
        ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_INCOMPLETE_FLASH, 0,
                      "incomplete OTA flash write");
        return;
    }

    if (!ota_verify_flash())
        return;

    if (!ota_switch_partition())
        return;

    ota.status = OTA_STATUS_COMPLETE;
    ota.active = false;
    ota.reboot_pending = true;
    ota.reboot_at_us = bflb_mtimer_get_time_us() + OTA_REBOOT_DELAY_US;
    LOG_INF("[OTA] Complete, reboot scheduled\n");
}

static void ota_abort(void)
{
    ota_update_init();
    LOG_INF("[OTA] Aborted\n");
}

bool ota_update_handle_command(const uint8_t *payload, uint32_t len)
{
    uint8_t cmd;

    if (!payload || len == 0)
        return false;

    cmd = payload[0];
    switch (cmd) {
    case OTA_CMD_START:
        if (len >= 5)
            ota_start(read_le32(payload + 1));
        else
            ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_SHORT_COMMAND, 0,
                          "short OTA start command");
        return true;

    case OTA_CMD_DATA:
        if (len >= 4 && payload[3] <= len - 4)
            ota_data((uint16_t)payload[1] | ((uint16_t)payload[2] << 8),
                     payload + 4, payload[3]);
        else
            ota_set_error(OTA_STATUS_ERR_SIGNATURE, OTA_ERR_SHORT_COMMAND, 0,
                          "short OTA data command");
        return true;

    case OTA_CMD_FINISH:
        ota_finish();
        return true;

    case OTA_CMD_ABORT:
        ota_abort();
        return true;

    default:
        return false;
    }
}

bool ota_update_fill_progress_report(uint8_t *buf, uint32_t len)
{
    uint32_t clear_len = len < 12 ? len : 12;

    if (!buf || len < 12)
        return false;

    if (clear_len > 3)
        memset(buf + 3, 0, clear_len - 3);

    if (ota.status == OTA_STATUS_IDLE && !ota.reboot_pending)
        return false;

    buf[3] = ota.status;
    write_le32(buf + 4, ota.package_received);
    write_le32(buf + 8, ota.package_size);

    return true;
}

bool ota_update_reboot_due(void)
{
    return ota.reboot_pending &&
           bflb_mtimer_get_time_us() >= ota.reboot_at_us;
}
