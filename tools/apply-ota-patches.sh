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

insert_after_fixed() {
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

insert_after_f6_command_var() {
    local snippet_file
    snippet_file="$(mktemp)"
    printf '%s\n' '        if (ota_update_handle_command(payload, payload_len)) {
            LOG_INF("[USB] CMD 0xF6/0x%02x: OTA\n", cmd);
            return;
        }
' > "$snippet_file"
    rewrite_with_awk src/usb_gamepad.c "OTA F6 command hook" '
        function print_snippet() {
            while ((getline line < snippet_file) > 0)
                print line
            close(snippet_file)
        }
        /report_id[[:space:]]*==[[:space:]]*0xF6/ {
            in_f6 = 1
        }
        in_f6 && /uint8_t[[:space:]]+cmd[[:space:]]*=[[:space:]]*payload\[0\];/ && !done {
            print
            print_snippet()
            done = 1
            next
        }
        { print }
        END { if (!done) exit 42 }
    ' -v snippet_file="$snippet_file"
    rm -f "$snippet_file"
}

copy_ota_sources() {
    mkdir -p src
    install -m 0644 "${OTA_OVERLAY_DIR}/src/ota_update.c" src/ota_update.c
    install -m 0644 "${OTA_OVERLAY_DIR}/src/ota_update.h" src/ota_update.h
}

ensure_cmake_hooks() {
    local need_source=0
    local need_include=0
    local need_force_include=0

    grep -Fq 'src/ota_update.c' CMakeLists.txt || need_source=1
    grep -Fq 'tinycrypt/include' CMakeLists.txt || need_include=1
    grep -Fq 'src/ota_update.h' CMakeLists.txt || need_force_include=1

    if [[ "$need_source" -eq 1 || "$need_include" -eq 1 || "$need_force_include" -eq 1 ]]; then
        {
            printf '\n# DS5Dongle OTA overlay\n'
            if [[ "$need_source" -eq 1 ]]; then
                printf '%s\n' 'target_sources(app PRIVATE src/ota_update.c)'
            fi
            if [[ "$need_include" -eq 1 ]]; then
                printf '%s\n' 'sdk_add_include_directories(${BL_SDK_BASE}/components/wireless/bluetooth/blestack/src/common/tinycrypt/include)'
            fi
            if [[ "$need_force_include" -eq 1 ]]; then
                printf '%s\n' 'target_compile_options(app PRIVATE $<$<COMPILE_LANGUAGE:C>:-include${CMAKE_CURRENT_LIST_DIR}/src/ota_update.h>)'
            fi
        } >> CMakeLists.txt
    fi
}

ensure_usb_gamepad_hooks() {
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

    if ! grep -Fq 'ota_update_fill_progress_report' src/usb_gamepad.c; then
        insert_after_fixed \
            src/usb_gamepad.c \
            "OTA progress feature report" \
            'feature_resp_buf[11] = 0;' \
            '            ota_update_fill_progress_report(feature_resp_buf, FEATURE_DATA_MAX + 1);'
    fi

    if ! grep -Fq 'ota_update_handle_command(payload, payload_len)' src/usb_gamepad.c; then
        insert_after_f6_command_var
    fi
}

ota_is_integrated() {
    [[ -f src/ota_update.c ]] &&
        [[ -f src/ota_update.h ]] &&
        grep -Fq 'src/ota_update.c' CMakeLists.txt &&
        grep -Fq 'tinycrypt/include' CMakeLists.txt &&
        grep -Fq 'src/ota_update.h' CMakeLists.txt &&
        grep -Fq 'ota_update_fill_progress_report' src/usb_gamepad.c &&
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
