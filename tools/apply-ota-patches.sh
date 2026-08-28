#!/usr/bin/env bash
set -euo pipefail

OVERLAY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${1:-$OVERLAY_DIR}"
OTA_OVERLAY_DIR="${2:-$OVERLAY_DIR/overlays/ota}"

to_abs_from_pwd() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s\n' "${PWD}/$1" ;;
    esac
}

to_abs_from_overlay() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s\n' "${OVERLAY_DIR}/$1" ;;
    esac
}

TARGET_DIR="$(to_abs_from_pwd "$TARGET_DIR")"
OTA_OVERLAY_DIR="$(to_abs_from_overlay "$OTA_OVERLAY_DIR")"

die() {
    echo "ERROR: $*" >&2
    exit 1
}

require_file() {
    [[ -f "$1" ]] || die "required file not found: $1"
}

rewrite_with_awk() {
    local file="$1"
    local desc="$2"
    local program="$3"
    shift 3

    local tmp
    tmp="$(mktemp)"
    if awk "$@" "$program" "$file" > "$tmp"; then
        mv "$tmp" "$file"
    else
        rm -f "$tmp"
        die "could not inject ${desc} into ${file}"
    fi
}

insert_after_pattern() {
    local file="$1"
    local desc="$2"
    local anchor="$3"
    local snippet="$4"
    local snippet_file
    snippet_file="$(mktemp)"
    printf '%s\n' "$snippet" > "$snippet_file"
    rewrite_with_awk "$file" "$desc" '
        function print_snippet() {
            while ((getline line < snippet_file) > 0)
                print line
            close(snippet_file)
        }
        $0 ~ anchor && !done {
            print
            print_snippet()
            done = 1
            next
        }
        { print }
        END { if (!done) exit 42 }
    ' -v anchor="$anchor" -v snippet_file="$snippet_file"
    rm -f "$snippet_file"
}

insert_before_pattern() {
    local file="$1"
    local desc="$2"
    local anchor="$3"
    local snippet="$4"
    local snippet_file
    snippet_file="$(mktemp)"
    printf '%s\n' "$snippet" > "$snippet_file"
    rewrite_with_awk "$file" "$desc" '
        function print_snippet() {
            while ((getline line < snippet_file) > 0)
                print line
            close(snippet_file)
        }
        $0 ~ anchor && !done {
            print_snippet()
            done = 1
        }
        { print }
        END { if (!done) exit 42 }
    ' -v anchor="$anchor" -v snippet_file="$snippet_file"
    rm -f "$snippet_file"
}

insert_before_fixed() {
    local file="$1"
    local desc="$2"
    local anchor="$3"
    local snippet="$4"
    local snippet_file
    snippet_file="$(mktemp)"
    printf '%s\n' "$snippet" > "$snippet_file"
    rewrite_with_awk "$file" "$desc" '
        function print_snippet() {
            while ((getline line < snippet_file) > 0)
                print line
            close(snippet_file)
        }
        index($0, anchor) && !done {
            print_snippet()
            done = 1
        }
        { print }
        END { if (!done) exit 42 }
    ' -v anchor="$anchor" -v snippet_file="$snippet_file"
    rm -f "$snippet_file"
}

copy_ota_sources() {
    mkdir -p src
    install -m 0644 "${OTA_OVERLAY_DIR}/src/ota_update.c" src/ota_update.c
    install -m 0644 "${OTA_OVERLAY_DIR}/src/ota_update.h" src/ota_update.h
}

ensure_cmake_hooks() {
    if ! grep -Fq 'src/ota_update.c' CMakeLists.txt; then
        rewrite_with_awk CMakeLists.txt "OTA source" '
            /target_sources[[:space:]]*\([[:space:]]*app[[:space:]]+PRIVATE/ {
                inside_sources = 1
            }
            inside_sources && /^[[:space:]]*\)[[:space:]]*$/ && !done {
                print "    src/ota_update.c"
                done = 1
                inside_sources = 0
            }
            { print }
            END { if (!done) exit 42 }
        '
    fi

    if ! grep -Fq 'tinycrypt/include' CMakeLists.txt; then
        insert_after_pattern \
            CMakeLists.txt \
            "tinycrypt include path" \
            'blestack/src/include' \
            'sdk_add_include_directories(${BL_SDK_BASE}/components/wireless/bluetooth/blestack/src/common/tinycrypt/include)'
    fi

    if ! grep -Fq 'FIRMWARE_VERSION_SUFFIX' CMakeLists.txt; then
        insert_before_pattern \
            CMakeLists.txt \
            "firmware version suffix compile definition" \
            '^target_compile_options' \
            'if(DEFINED ENV{FW_VERSION_SUFFIX})
    target_compile_definitions(app PRIVATE FIRMWARE_VERSION_SUFFIX=\"$ENV{FW_VERSION_SUFFIX}\")
    message(STATUS "Firmware version suffix: $ENV{FW_VERSION_SUFFIX}")
endif()'
    fi
}

ensure_usb_gamepad_hooks() {
    if ! grep -Fq '#include "ota_update.h"' src/usb_gamepad.c; then
        insert_after_pattern \
            src/usb_gamepad.c \
            "OTA header include" \
            '^#include "remap.h"$' \
            '#include "ota_update.h"'
    fi

    if ! grep -Fq '#ifndef FIRMWARE_VERSION_SUFFIX' src/usb_gamepad.c; then
        insert_before_pattern \
            src/usb_gamepad.c \
            "firmware version suffix fallback" \
            '^[[:space:]]*#if defined' \
            '#ifndef FIRMWARE_VERSION_SUFFIX
#define FIRMWARE_VERSION_SUFFIX ""
#endif
'
    fi

    perl -0pi -e 's/(#define\s+FIRMWARE_VERSION\s+"[^"\n]*")(?!\s*FIRMWARE_VERSION_SUFFIX)/$1 FIRMWARE_VERSION_SUFFIX/g' src/usb_gamepad.c
    if grep -Eq '#define[[:space:]]+FIRMWARE_VERSION[[:space:]]+"[^"]+"[[:space:]]*$' src/usb_gamepad.c; then
        die "some FIRMWARE_VERSION definitions do not include FIRMWARE_VERSION_SUFFIX"
    fi

    if ! grep -Fq 'ota_update_reboot_due()' src/usb_gamepad.c; then
        insert_before_pattern \
            src/usb_gamepad.c \
            "OTA reboot polling" \
            '^[[:space:]]*uint8_t frid = pending_feature_rid;' \
            '    if (ota_update_reboot_due()) {
        LOG_INF("[USB] OTA reboot\n");
        vTaskDelay(pdMS_TO_TICKS(100));
        GLB_SW_System_Reset();
    }
'
    fi

    if ! grep -Fq 'ota_update_init();' src/usb_gamepad.c; then
        insert_before_pattern \
            src/usb_gamepad.c \
            "OTA initialization" \
            'USB-INIT' \
            '    ota_update_init();
'
    fi

    if ! grep -Fq 'ota_update_fill_progress_report' src/usb_gamepad.c; then
        insert_before_fixed \
            src/usb_gamepad.c \
            "OTA progress feature report" \
            'feature_resp_buf[12] = get_battery_level();' \
            '            memset(feature_resp_buf + 3, 0, 9);
            ota_update_fill_progress_report(feature_resp_buf, FEATURE_DATA_MAX + 1);'
    fi

    if ! awk '
        /ota_update_fill_progress_report/ { seen_ota = 1 }
        seen_ota && /\*len[[:space:]]*=[[:space:]]*24[[:space:]]*;/ { found = 1 }
        END { exit found ? 0 : 1 }
    ' src/usb_gamepad.c; then
        rewrite_with_awk src/usb_gamepad.c "OTA progress report length" '
            /ota_update_fill_progress_report/ { seen_ota = 1 }
            seen_ota && /\*len[[:space:]]*=[[:space:]]*14[[:space:]]*;/ && !done {
                sub(/\*len[[:space:]]*=[[:space:]]*14[[:space:]]*;/, "*len  = 24;")
                done = 1
            }
            { print }
            END { if (!done) exit 42 }
        '
    fi

    if ! grep -Fq 'ota_update_handle_command(payload, payload_len)' src/usb_gamepad.c; then
        perl -0pi -e 's/if \(\s*cmd\s*==\s*0x01\s*&&\s*payload_len\s*>\s*1\s*\) \{/if (ota_update_handle_command(payload, payload_len)) {\n            LOG_INF("[USB] CMD 0xF6\/0x%02x: OTA\\n", cmd);\n        } else if (cmd == 0x01 \&\& payload_len > 1) {/s' src/usb_gamepad.c
    fi
}

ota_is_integrated() {
    [[ -f src/ota_update.c ]] &&
        [[ -f src/ota_update.h ]] &&
        grep -Fq 'src/ota_update.c' CMakeLists.txt &&
        grep -Fq 'tinycrypt/include' CMakeLists.txt &&
        grep -Fq 'FIRMWARE_VERSION_SUFFIX' CMakeLists.txt &&
        grep -Fq '#include "ota_update.h"' src/usb_gamepad.c &&
        grep -Fq 'ota_update_init();' src/usb_gamepad.c &&
        grep -Fq 'ota_update_fill_progress_report' src/usb_gamepad.c &&
        grep -Eq '\*len[[:space:]]*=[[:space:]]*24[[:space:]]*;' src/usb_gamepad.c &&
        grep -Fq 'ota_update_handle_command(payload, payload_len)' src/usb_gamepad.c &&
        grep -Fq 'ota_update_reboot_due()' src/usb_gamepad.c
}

verify_ota_integration() {
    if ! ota_is_integrated; then
        die "OTA integration is incomplete after overlay injection"
    fi
    echo "OTA integration verified."
}

cd "$TARGET_DIR"
require_file CMakeLists.txt
require_file src/usb_gamepad.c
require_file "${OTA_OVERLAY_DIR}/src/ota_update.c"
require_file "${OTA_OVERLAY_DIR}/src/ota_update.h"

copy_ota_sources
ensure_cmake_hooks
ensure_usb_gamepad_hooks
verify_ota_integration
