#!/bin/sh
# Compila o Kalango Audio e instala para o usuário atual (sem sudo).
# Uso: ./install.sh
set -e
cd "$(dirname "$0")"
cargo build --release --locked
BIN="$HOME/.local/bin"
APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor"
mkdir -p "$BIN" "$APPS"
install -m 755 target/release/kalango-audio "$BIN/kalango-audio"
for s in 128 256 512; do
    mkdir -p "$ICONS/${s}x${s}/apps"
    install -m 644 "assets/icons/hicolor/${s}x${s}/apps/os.kalango.Audio.png" "$ICONS/${s}x${s}/apps/os.kalango.Audio.png"
done
sed "s|^Exec=.*|Exec=$BIN/kalango-audio|" dist/os.kalango.Audio.desktop > "$APPS/os.kalango.Audio.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" || true
echo "Instalado. Procure \"Kalango Audio\" nos aplicativos."
