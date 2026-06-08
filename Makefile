.PHONY: build release install uninstall clean run help

BINARY_NAME = hikvision-rs
INSTALL_DIR ?= /usr/local/bin
BUILD_DIR = target/release

help:
	@echo "hikvision-rs - Visualizador RTSP para DVRs Hikvision"
	@echo ""
	@echo "Targets disponíveis:"
	@echo "  make build      - Build debug (recomendado para desenvolvimento)"
	@echo "  make release    - Build release com LTO"
	@echo "  make install    - Instalar binário em $(INSTALL_DIR) (requer sudo)"
	@echo "  make uninstall  - Remover binário de $(INSTALL_DIR) (requer sudo)"
	@echo "  make clean      - Limpar artefatos de build"
	@echo "  make run        - Executar em modo debug"
	@echo "  make run-release - Executar em modo release"
	@echo ""
	@echo "Variáveis de ambiente:"
	@echo "  INSTALL_DIR=/caminho/custom - Diretório de instalação (padrão: /usr/local/bin)"

build:
	cargo build

release:
	cargo build --release

install:
	@echo "Instalando $(BINARY_NAME) em $(INSTALL_DIR)..."
	@mkdir -p $(INSTALL_DIR)
	install -m 755 $(BUILD_DIR)/$(BINARY_NAME) $(INSTALL_DIR)/$(BINARY_NAME)
	@echo "Instalado em $(INSTALL_DIR)/$(BINARY_NAME)"

uninstall:
	@echo "Removendo $(BINARY_NAME) de $(INSTALL_DIR)..."
	rm -f $(INSTALL_DIR)/$(BINARY_NAME)
	@echo "Removido."

clean:
	cargo clean

run:
	cargo run

run-release:
	cargo run --release