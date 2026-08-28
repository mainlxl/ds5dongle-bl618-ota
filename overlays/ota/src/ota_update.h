#ifndef OTA_UPDATE_H
#define OTA_UPDATE_H

#include <stdbool.h>
#include <stdint.h>

enum ota_update_status {
    OTA_STATUS_IDLE = 0,
    OTA_STATUS_RECEIVING = 1,
    OTA_STATUS_VERIFYING = 2,
    OTA_STATUS_COMPLETE = 3,
    OTA_STATUS_ERR_GENERIC = 224,
    OTA_STATUS_ERR_CRC = 225,
    OTA_STATUS_ERR_SIGNATURE = 226,
    OTA_STATUS_ERR_DEVICE = 227,
    OTA_STATUS_ERR_DECRYPT = 228,
    OTA_STATUS_ERROR = 255,
};

enum ota_update_error_detail {
    OTA_ERR_NONE = 0,
    OTA_ERR_PARTITION = 1,
    OTA_ERR_ACTIVE_TABLE = 2,
    OTA_ERR_FW_ENTRY = 3,
    OTA_ERR_PARTITION_SWITCH = 4,
    OTA_ERR_BAD_MAGIC = 5,
    OTA_ERR_BAD_TYPE = 6,
    OTA_ERR_BAD_SIZE = 7,
    OTA_ERR_IMAGE_TOO_LARGE = 8,
    OTA_ERR_FLASH_ERASE = 9,
    OTA_ERR_FLASH_WRITE = 10,
    OTA_ERR_FLASH_READBACK = 11,
    OTA_ERR_FLASH_COMPARE = 12,
    OTA_ERR_TOO_MUCH_PAYLOAD = 13,
    OTA_ERR_PAYLOAD_MAGIC = 14,
    OTA_ERR_SHA_INIT = 15,
    OTA_ERR_SHA_UPDATE = 16,
    OTA_ERR_SHA_FINISH = 17,
    OTA_ERR_SHA_MISMATCH = 18,
    OTA_ERR_PACKAGE_TOO_SMALL = 19,
    OTA_ERR_INCOMPLETE_PACKAGE = 20,
    OTA_ERR_INCOMPLETE_FLASH = 21,
    OTA_ERR_SEQUENCE = 22,
    OTA_ERR_SHORT_COMMAND = 23,
};

void ota_update_init(void);
bool ota_update_handle_command(const uint8_t *payload, uint32_t len);
bool ota_update_fill_progress_report(uint8_t *buf, uint32_t len);
bool ota_update_reboot_due(void);

#endif /* OTA_UPDATE_H */
