#!/bin/sh
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIBS_DIR="${SCRIPT_DIR}/hikvision-libs"
SDK_PATTERN="EN-HCNetSDKV"

echo "=============================================="
echo " Hikvision Device Network SDK Installer"
echo "=============================================="
echo ""

mkdir -p "$LIBS_DIR"

# Find any archive already in hikvision-libs (.zip or .tgz)
SDK_ARCHIVE=$(find "${LIBS_DIR}" -maxdepth 1 \( -name "*.zip" -o -name "*.tgz" -o -name "*.tar.gz" \) 2>/dev/null | head -1)

if [ -f "${LIBS_DIR}/libPlayCtrl.so" ] && [ -f "${LIBS_DIR}/libhcnetsdk.so" ]; then
    echo "SDK already installed at: ${LIBS_DIR}"
    echo ""
    echo "Available files:"
    ls -lh "${LIBS_DIR}"/*.so 2>/dev/null | awk '{printf "  %-30s %s\n", $9, $5}' | sed "s|${LIBS_DIR}/||"
    echo ""
    exit 0
fi

if [ -z "$SDK_ARCHIVE" ]; then
    echo "No SDK archive found in: ${LIBS_DIR}"
    echo ""
    echo "The Hikvision website blocks automated downloads."
    echo "Please download manually:"
    echo ""
    echo "  1. Go to: https://www.hikvision.com/en/support/download/sdk/"
    echo "  2. Select: Product Type = Camera / DVR / NVR"
    echo "  3. Select: Download Type = Device Network SDK"
    echo "  4. Choose: Linux 64-bit"
    echo "  5. Click 'Download' and save the file"
    echo "     (e.g. EN-HCNetSDKV6.1.9.48_build20230410_linux64.zip)"
    echo ""
    echo "  6. Move the .zip file to: ${LIBS_DIR}/"
    echo "  7. Run this script again"
    echo ""
    echo "Example:"
    echo "  mv ~/Downloads/EN-HCNetSDKV*.zip ${LIBS_DIR}/"
    echo "  ./sdkdownload.sh"
    echo ""
    exit 1
fi

echo "Found: $(basename "$SDK_ARCHIVE")"
echo ""
echo "Extracting..."
cd "$LIBS_DIR"

case "$(basename "$SDK_ARCHIVE")" in
    *.zip)
        unzip -o "$SDK_ARCHIVE"
        ;;
    *.tgz|*.tar.gz)
        tar -xzf "$SDK_ARCHIVE"
        ;;
esac
rm -f "$SDK_ARCHIVE"

# Find SDK extracted folder
SDK_FOLDER=$(find . -maxdepth 2 -type d -name "${SDK_PATTERN}*" 2>/dev/null | head -1)

if [ -z "$SDK_FOLDER" ]; then
    echo "Warning: no folder matching ${SDK_PATTERN}* found"
    echo "Searching for libraries manually..."
    find . -name "libPlayCtrl.so" -o -name "libhcnetsdk.so" 2>/dev/null
else
    echo "SDK found at: ${SDK_FOLDER}"
fi

# Copy all .so files from lib/ to hikvision-libs root
if [ -d "$SDK_FOLDER/lib" ]; then
    echo "Copying libraries..."
    cp -f "${SDK_FOLDER}/lib/"*.so . 2>/dev/null || true

    # Copy HCNetSDKCom components
    if [ -d "${SDK_FOLDER}/lib/HCNetSDKCom" ]; then
        cp -r "${SDK_FOLDER}/lib/HCNetSDKCom" . 2>/dev/null || true
    fi

    # Copy Qt5 deps if present (libcrypto, libssl, libopenal)
    for lib in libcrypto.so* libssl.so* libopenal.so*; do
        cp -f "${SDK_FOLDER}/lib/$lib" . 2>/dev/null || true
    done
fi

echo ""
echo "=============================================="
echo " SDK installed at: ${LIBS_DIR}"
echo "=============================================="
echo ""
echo "Available files:"
ls -lh *.so 2>/dev/null | awk '{printf "  %-30s %s\n", $9, $5}' | sed "s|${LIBS_DIR}/||"
echo ""

# Check HCNetSDKCom
if [ -d "HCNetSDKCom" ]; then
    COM_COUNT=$(ls HCNetSDKCom/*.so 2>/dev/null | wc -l)
    echo "HCNetSDKCom/: ${COM_COUNT} components"
fi

echo ""
echo "Done! You can now run:"
echo "  cargo build --release"
echo ""
