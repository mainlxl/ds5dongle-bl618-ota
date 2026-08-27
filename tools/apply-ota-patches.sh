#!/usr/bin/env bash
set -euo pipefail

OVERLAY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${1:-$OVERLAY_DIR}"
PATCH_DIR="${2:-$OVERLAY_DIR/patches/ota}"

case "$TARGET_DIR" in
    /*) ;;
    *) TARGET_DIR="${PWD}/${TARGET_DIR}" ;;
esac

case "$PATCH_DIR" in
    /*) ;;
    *) PATCH_DIR="${OVERLAY_DIR}/${PATCH_DIR}" ;;
esac

cd "$TARGET_DIR"

ota_is_integrated() {
    [[ -f src/ota_update.c ]] &&
        [[ -f src/ota_update.h ]] &&
        grep -q 'src/ota_update.c' CMakeLists.txt &&
        grep -q '#include "ota_update.h"' src/usb_gamepad.c &&
        grep -q 'ota_update_init();' src/usb_gamepad.c &&
        grep -q 'ota_update_fill_progress_report' src/usb_gamepad.c &&
        grep -q 'ota_update_handle_command(payload, payload_len)' src/usb_gamepad.c
}

verify_ota_integration() {
    if ! ota_is_integrated; then
        echo "ERROR: OTA integration is incomplete after patch application." >&2
        echo "Expected src/ota_update.c/h plus usb_gamepad.c and CMakeLists.txt hook points." >&2
        exit 1
    fi

    echo "OTA integration verified."
}

if ota_is_integrated; then
    verify_ota_integration
    exit 0
fi

shopt -s nullglob
patches=("${PATCH_DIR}"/*.patch)
if [[ "${#patches[@]}" -eq 0 ]]; then
    echo "ERROR: no OTA patches found in ${PATCH_DIR}." >&2
    exit 1
fi

for patch in "${patches[@]}"; do
    echo "Applying OTA patch: ${patch}"
    if git apply --check "$patch"; then
        git apply "$patch"
    else
        git apply --3way "$patch"
    fi
done

verify_ota_integration
