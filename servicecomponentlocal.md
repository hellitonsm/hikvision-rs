# Hikvision Web Local Server — Instalação Local (sem root)

Guia para extrair e executar o `LocalServiceComponent_amd64_uos.deb` como serviço **do usuário**, sem instalar nada como root.

## Pré-requisitos

- Sistema com `systemd --user` (qualquer Linux moderno)
- `dpkg-deb` disponível (para extrair o .deb)
- Sessão gráfica ativa (X11/Wayland) — o binário depende de display

## Instalação

```bash
# 1. Extrair o .deb
mkdir -p /tmp/hikv-extract
dpkg-deb -x LocalServiceComponent_amd64_uos.deb /tmp/hikv-extract
mkdir -p ~/.local/share/hikvision/weblocalserver
mv /tmp/hikv-extract/opt/apps/com.hikvision.weblocalserver/* ~/.local/share/hikvision/weblocalserver/
rm -rf /tmp/hikv-extract

# 2. Criar wrapper de ambiente
cat > ~/.local/share/hikvision/weblocalserver/start.sh << 'EOF'
#!/bin/bash
DIR="$HOME/.local/share/hikvision/weblocalserver"
export LD_LIBRARY_PATH="$DIR/files/lib:$DIR/files/bin:${LD_LIBRARY_PATH:-}"
export QT_PLUGIN_PATH="$DIR/files/plugins"
cd "$DIR/files/bin" || exit 1
exec ./LocalServiceControl "$@"
EOF
chmod +x ~/.local/share/hikvision/weblocalserver/start.sh

# 3. Criar systemd user service
mkdir -p ~/.config/systemd/user
cat > ~/.config/systemd/user/hikvision-weblocalserver.service << 'EOF'
[Unit]
Description=Hikvision Web Local Server
After=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.local/share/hikvision/weblocalserver/start.sh
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF

# 4. Ativar e iniciar
systemctl --user daemon-reload
systemctl --user start hikvision-weblocalserver
systemctl --user status hikvision-weblocalserver
```

## Gerenciamento

```bash
# Iniciar
systemctl --user start hikvision-weblocalserver

# Parar
systemctl --user stop hikvision-weblocalserver

# Reiniciar
systemctl --user restart hikvision-weblocalserver

# Status
systemctl --user status hikvision-weblocalserver

# Logs em tempo real
journalctl --user -u hikvision-weblocalserver -f

# Auto-início no login (se desejar)
systemctl --user enable hikvision-weblocalserver
```

## Estrutura de diretórios

```
~/.local/share/hikvision/weblocalserver/
├── info                        # Metadados do app
├── entries/                    # .desktop files (não usado pelo serviço)
├── files/
│   ├── bin/
│   │   ├── LocalServiceControl # Binário principal
│   │   ├── lib*.so             # Libs proprietárias Hikvision
│   │   ├── libtufao1.so.1.3.10 # Tufao HTTP library
│   │   ├── libcrypto.so.1.1    # OpenSSL
│   │   ├── libssl.so.1.1       # OpenSSL
│   │   └── qt.conf             # Config Qt
│   ├── lib/
│   │   ├── libQt5*.so.5        # Qt5 bundled
│   │   └── libicu*.so.56       # ICU 56 bundled
│   └── plugins/                # Qt5 plugins
├── start.sh                    # Wrapper script
└── guia-instalacao.md          # Este guia

~/.config/systemd/user/
└── hikvision-weblocalserver.service
```

## Observações

- **Nada é instalado como root.** Tudo fita em `~/.local/share/` e `~/.config/systemd/user/`.
- O binário usa `127.0.0.1` (loopback) — não exposto na rede.
- O aviso `Cannot load libcuda.so.1` é normal (GPU NVIDIA ausente) e não afeta o funcionamento.
- O serviço depende de `graphical-session.target` — só funciona com sessão gráfica ativa.

## Desinstalação

```bash
systemctl --user stop hikvision-weblocalserver
systemctl --user disable hikvision-weblocalserver
rm ~/.config/systemd/user/hikvision-weblocalserver.service
systemctl --user daemon-reload
rm -rf ~/.local/share/hikvision/weblocalserver
```
