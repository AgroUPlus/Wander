#!/usr/bin/env bash
# Build wander and install it so it can be launched by name or from a
# desktop launcher, with no PATH changes needed.
set -euo pipefail

cd "$(dirname "$0")"

# Target PREFIX: /usr if run via sudo, ~/.local if run as regular user
if [ "$(id -u)" -eq 0 ]; then
  PREFIX="${PREFIX:-/usr}"
else
  PREFIX="${PREFIX:-${HOME}/.local}"
fi

BIN_DIR="${BIN_DIR:-${PREFIX}/bin}"
APP_DIR="${APP_DIR:-${PREFIX}/share/applications}"
ICON_DIR="${ICON_DIR:-${PREFIX}/share/icons/hicolor/scalable/apps}"

# Locate cargo for current user or SUDO_USER
find_cargo() {
  if command -v cargo >/dev/null 2>&1; then
    command -v cargo
    return 0
  fi
  if [ -f "$HOME/.cargo/env" ]; then
    . "$HOME/.cargo/env" 2>/dev/null || true
  fi
  if command -v cargo >/dev/null 2>&1; then
    command -v cargo
    return 0
  fi
  if [ -x "$HOME/.cargo/bin/cargo" ]; then
    echo "$HOME/.cargo/bin/cargo"
    return 0
  fi
  if [ -n "${SUDO_USER:-}" ]; then
    local user_home
    user_home=$(eval echo "~$SUDO_USER")
    if [ -x "$user_home/.cargo/bin/cargo" ]; then
      echo "$user_home/.cargo/bin/cargo"
      return 0
    fi
  fi
  return 1
}

# Always build as a regular user (never root) to preserve target/ permissions
echo "==> Building wander (release)"
CARGO_EXEC=$(find_cargo || echo "cargo")
CARGO_DIR="$(dirname "$CARGO_EXEC")"

if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ]; then
  sudo -u "$SUDO_USER" env PATH="$CARGO_DIR:$PATH" "$CARGO_EXEC" build --release
else
  export PATH="$CARGO_DIR:$PATH"
  "$CARGO_EXEC" build --release
fi

# Escalate to sudo ONLY for the copy/install phase if destination is not writable
if [ "$(id -u)" -ne 0 ] && ! [ -w "$BIN_DIR" 2>/dev/null ] && ! [ -w "$(dirname "$BIN_DIR")" 2>/dev/null ]; then
  echo "==> Installing to ${BIN_DIR} requires elevated permissions."
  echo "    Running installation phase with sudo..."
  exec sudo PREFIX="$PREFIX" BIN_DIR="$BIN_DIR" APP_DIR="$APP_DIR" ICON_DIR="$ICON_DIR" "$0" "$@"
fi

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
  gtk-update-icon-cache -f -t "${PREFIX}/share/icons/hicolor" >/dev/null 2>&1 || true
else
  touch "${PREFIX}/share/icons/hicolor" >/dev/null 2>&1 || true
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
    echo "      set -gx PATH ${BIN_DIR} \$PATH   # fish"
    echo "      export PATH=\"${BIN_DIR}:\$PATH\"  # bash/zsh"
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
