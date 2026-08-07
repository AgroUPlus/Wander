#!/usr/bin/env bash
# Build wander and install it so it can be launched by name or from a
# desktop launcher, with no PATH changes needed.
set -euo pipefail

cd "$(dirname "$0")"

# Ensure cargo is in PATH if installed in standard ~/.cargo location
if ! command -v cargo >/dev/null 2>&1; then
  if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
  elif [ -d "$HOME/.cargo/bin" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
fi

BIN_DIR="${HOME}/.local/bin"
APP_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/scalable/apps"

echo "==> Building (release)"
cargo build --release

echo "==> Installing binary to ${BIN_DIR}"
mkdir -p "$BIN_DIR"
install -m 755 target/release/wander "${BIN_DIR}/wander"

echo "==> Installing desktop entry"
mkdir -p "$APP_DIR" "$ICON_DIR"
sed "s|^Exec=wander|Exec=${BIN_DIR}/wander|" packaging/wander.desktop > "${APP_DIR}/wander.desktop"
chmod 644 "${APP_DIR}/wander.desktop"
install -m 644 packaging/wander.svg "${ICON_DIR}/wander.svg"

# Refresh the launcher's cache so the entry and icon show up immediately.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" >/dev/null 2>&1 || true
else
  touch "${HOME}/.local/share/icons/hicolor" >/dev/null 2>&1 || true
fi

echo
echo "Installed."

case ":${PATH}:" in
  *":${BIN_DIR}:"*)
    echo "  Run it with:  wander"
    ;;
  *)
    echo "  NOTE: ${BIN_DIR} is not on your PATH."
    echo "  Add this to your shell config:"
    echo "      set -gx PATH \$HOME/.local/bin \$PATH   # fish"
    echo "      export PATH=\"\$HOME/.local/bin:\$PATH\"  # bash/zsh"
    ;;
esac

cat <<'EOF'

Optional Hyprland integration — add to ~/.config/hypr/hyprland.conf:

    # Float wander as a centred scratchpad-style window
    windowrulev2 = float, class:^(wander)$
    windowrulev2 = size 60% 70%, class:^(wander)$
    windowrulev2 = center, class:^(wander)$

    # Super+M toggles it
    bind = SUPER, M, exec, pgrep -x wander >/dev/null && hyprctl dispatch closewindow class:wander || foot -a wander wander

Quickstart setup wizard:

    wander --quickstart

Or configure manually at ~/.config/wander/config.toml:

    [server]
    url = "https://navidrome.example.com"
    username = "you"

Then store the password in your keyring (it is never written to disk):

    wander --set-password
EOF
