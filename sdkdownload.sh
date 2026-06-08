#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIBS_DIR="${SCRIPT_DIR}/hikvision-libs"
SDK_URL="${1:-}"

if [ -z "$SDK_URL" ]; then
    echo "=============================================="
    echo " Baixador do Hikvision Device Network SDK"
    echo "=============================================="
    echo ""
    echo "Você precisa obter o link de download manualmente:"
    echo ""
    echo "  1. Acesse: https://www.hikvision.com/en/support/download/sdk/"
    echo "  2. Selecione: Product Type = Camera / DVR / NVR"
    echo "  3. Selecione: Download Type = Device Network SDK"
    echo "  4. Escolha: Linux 64-bit"
    echo "  5. Clique em 'Download' e copie o link do arquivo"
    echo ""
    echo "Uso:"
    echo "  $0 <URL_DO_SDK>"
    echo ""
    echo "Exemplo:"
    echo "  $0 https://assets.hikvision.com/prd/normal/all/files/202605/EN-HCNetSDKV6.1.9.48_build20230410_linux64.zip"
    echo ""
    exit 1
fi

echo "Baixando SDK..."
mkdir -p "$LIBS_DIR"
cd "$LIBS_DIR"

curl -L -o sdk.tgz "$SDK_URL"

echo "Extraindo..."
tar -xzf sdk.tgz
rm sdk.tgz

# Find and move libs to hikvision-libs root
if [ -d "lib" ]; then
    cp lib/*.so . 2>/dev/null || true
    cp lib/HCNetSDKCom/*.so . 2>/dev/null || true
fi

echo ""
echo "=============================================="
echo " SDK instalado em: $LIBS_DIR"
echo "=============================================="
echo ""
echo "Arquivos disponíveis:"
ls -lh *.so 2>/dev/null | awk '{print "  " $9}' || echo "  Nenhum .so encontrado"
echo ""
echo "Para instalar junto ao binário (make install):"
echo "  cp *.so ~/.config/hikvision-rs/"
echo "  sudo make install"
echo ""
