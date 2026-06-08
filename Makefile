.PHONY: build release install uninstall clean run help

BINARY_NAME = hikvision-rs
INSTALL_DIR ?= /usr/local/bin
DESKTOP_DIR ?= /usr/share/applications
ICON_DIR ?= /usr/share/icons/hicolor/scalable/apps
BUILD_DIR = target/release

help:
	@echo "hikvision-rs - Visualizador RTSP para DVRs Hikvision"
	@echo ""
	@echo "Targets disponíveis:"
	@echo "  make build       - Build debug (recomendado para desenvolvimento)"
	@echo "  make release     - Build release com LTO"
	@echo "  make install     - Instalar binário, ícone e .desktop (requer sudo)"
	@echo "  make uninstall   - Remover instalação completa (requer sudo)"
	@echo "  make clean       - Limpar artefatos de build"
	@echo "  make run         - Executar em modo debug"
	@echo "  make run-release - Executar em modo release"
	@echo ""
	@echo "Variáveis de ambiente:"
	@echo "  INSTALL_DIR=/caminho/custom  - Diretório do binário (padrão: /usr/local/bin)"
	@echo "  DESKTOP_DIR=/caminho/custom  - Diretório .desktop (padrão: /usr/share/applications)"
	@echo "  ICON_DIR=/caminho/custom    - Diretório ícones (padrão: /usr/share/icons/hicolor/scalable/apps)"

build:
	cargo build

release:
	cargo build --release

install:
	@echo "Instalando $(BINARY_NAME)..."
	@mkdir -p $(INSTALL_DIR)
	install -m 755 $(BUILD_DIR)/$(BINARY_NAME) $(INSTALL_DIR)/$(BINARY_NAME)
	@echo "  Binário: $(INSTALL_DIR)/$(BINARY_NAME)"
	@mkdir -p $(ICON_DIR)
	install -m 644 assets/hikvision-rs.svg $(ICON_DIR)/hikvision-rs.svg
	@echo "  Ícone:  $(ICON_DIR)/hikvision-rs.svg"
	@mkdir -p $(DESKTOP_DIR)
	install -m 644 assets/hikvision-rs.desktop $(DESKTOP_DIR)/hikvision-rs.desktop
	@echo "  Desktop: $(DESKTOP_DIR)/hikvision-rs.desktop"
	@if [ -d "hikvision-libs" ]; then \
		mkdir -p /usr/local/lib; \
		for lib in $(BUILD_DIR)/hikvision-libs/*.so; do \
			[ -f "$$lib" ] && install -m 644 "$$lib" /usr/local/lib/ && echo "  Lib:    /usr/local/lib/$$(basename $$lib)"; \
		done; \
	elif [ -d "$(HOME)/.config/hikvision-rs" ]; then \
		mkdir -p /usr/local/lib; \
		for lib in $(HOME)/.config/hikvision-rs/*.so; do \
			[ -f "$$lib" ] && install -m 644 "$$lib" /usr/local/lib/ && echo "  Lib:    /usr/local/lib/$$(basename $$lib)"; \
		done; \
	else \
		echo "  Libs:   Nenhuma lib encontrada em hikvision-libs/ ou ~/.config/hikvision-rs/"; \
	fi
	@ldconfig 2>/dev/null || true
	@update-desktop-database $(DESKTOP_DIR) 2>/dev/null || true
	@gtk-update-icon-cache -f /usr/share/icons/hicolor 2>/dev/null || true
	@echo "Instalação concluída."

uninstall:
	@echo "Removendo $(BINARY_NAME)..."
	rm -f $(INSTALL_DIR)/$(BINARY_NAME)
	rm -f $(ICON_DIR)/hikvision-rs.svg
	rm -f $(DESKTOP_DIR)/hikvision-rs.desktop
	@for lib in libhcnetsdk.so libPlayCtrl.so libAudioRender.so libSuperRender.so; do \
		rm -f /usr/local/lib/$$lib; \
	done
	@ldconfig 2>/dev/null || true
	@update-desktop-database $(DESKTOP_DIR) 2>/dev/null || true
	@gtk-update-icon-cache -f /usr/share/icons/hicolor 2>/dev/null || true
	@echo "Remoção concluída."

clean:
	cargo clean

run:
	cargo run

run-release:
	cargo run --release