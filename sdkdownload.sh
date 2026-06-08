#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIBS_DIR="${SCRIPT_DIR}/hikvision-libs"
SDK_PATTERN="EN-HCNetSDKV"

echo "=============================================="
echo " Baixador do Hikvision Device Network SDK"
echo "=============================================="
echo ""

# Check if SDK is already extracted
if [ -f "${LIBS_DIR}/libPlayCtrl.so" ] && [ -f "${LIBS_DIR}/libhcnetsdk.so" ]; then
    echo "SDK já instalado em: ${LIBS_DIR}"
    echo ""
    echo "Arquivos disponíveis:"
    ls -lh "${LIBS_DIR}"/*.so 2>/dev/null | awk '{printf "  %-30s %s\n", $9, $5}' | sed "s|${LIBS_DIR}/||"
    echo ""
    read -p "Deseja baixar novamente? (n/N): " -n 1 -r; echo
    if [[ ! $REPLY =~ ^[Ss]$ ]]; then
        echo "Abortado."
        exit 0
    fi
fi

echo "Baixando SDK Hikvision..."
echo ""
echo "Você precisa obter o link de download manualmente:"
echo ""
echo "  1. Acesse: https://www.hikvision.com/en/support/download/sdk/"
echo "  2. Selecione: Product Type = Camera / DVR / NVR"
echo "  3. Selecione: Download Type = Device Network SDK"
echo "  4. Escolha: Linux 64-bit"
echo "  5. Clique em 'Download' e copie o link do arquivo"
echo ""
read -p "Cole a URL do download: " SDK_URL

if [ -z "$SDK_URL" ]; then
    echo "URL não fornecida. Abortando."
    exit 1
fi

echo ""
echo "Baixando: $SDK_URL"
mkdir -p "$LIBS_DIR"
cd "$LIBS_DIR"

# Download with progress
curl -L -o sdk.tgz "$SDK_URL" --progress-bar

echo ""
echo "Extraindo..."
tar -xzf sdk.tgz
rm -f sdk.tgz

# Find SDK extracted folder
SDK_FOLDER=$(find . -maxdepth 2 -type d -name "${SDK_PATTERN}*" 2>/dev/null | head -1)

if [ -z "$SDK_FOLDER" ]; then
    echo "AVISO: Não encontrou pasta com padrão ${SDK_PATTERN}*"
    echo "Procurando libs manualmente..."
    find . -name "libPlayCtrl.so" -o -name "libhcnetsdk.so" 2>/dev/null
else
    echo "SDK encontrado em: ${SDK_FOLDER}"
fi

# Copy all .so files from lib/ to hikvision-libs root
if [ -d "$SDK_FOLDER/lib" ]; then
    echo "Copiando libs..."
    cp -f "${SDK_FOLDER}/lib/"*.so . 2>/dev/null || true

    # Copy HCNetSDKCom components
    if [ -d "${SDK_FOLDER}/lib/HCNetSDKCom" ]; then
        cp -r "${SDK_FOLDER}/lib/HCNetSDKCom" . 2>/dev/null || true
    fi

    # Copy Qt5 deps if exists (libcrypto, libssl, libopenal)
    for lib in libcrypto.so* libssl.so* libopenal.so*; do
        cp -f "${SDK_FOLDER}/lib/$lib" . 2>/dev/null || true
    done
fi

echo ""
echo "=============================================="
echo " SDK instalado em: ${LIBS_DIR}"
echo "=============================================="
echo ""
echo "Arquivos disponíveis:"
ls -lh *.so 2>/dev/null | awk '{printf "  %-30s %s\n", $9, $5}' | sed "s|${LIBS_DIR}/||"
echo ""

# Check HCNetSDKCom
if [ -d "HCNetSDKCom" ]; then
    COM_COUNT=$(ls HCNetSDKCom/*.so 2>/dev/null | wc -l)
    echo "HCNetSDKCom/: ${COM_COUNT} componentes"
fi

echo ""
echo "Pronto! Você pode usar:"
echo "  sudo make install    # instala o app + libs automaticamente"
echo ""