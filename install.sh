#!/usr/bin/env bash
# Installs (or updates) Now Playing Flagger on Linux from the latest GitHub release.
#
#   bash install.sh               install or update
#   bash install.sh --uninstall   remove it
#   bash install.sh --file PATH   install a local .deb or .AppImage instead of downloading
#
# Debian/Ubuntu/Mint (anything with apt) get the .deb, installed system-wide via apt so its
# dependencies come along. Everything else gets the AppImage, installed for the current user
# only (no sudo) with an app-menu entry.
set -euo pipefail

REPO="v6belung/MachinesCantSing"
APP="now-playing-flagger"
NAME="Now Playing Flagger"

BIN_DIR="$HOME/.local/bin"
APPIMAGE_PATH="$BIN_DIR/$APP.AppImage"
DESKTOP_FILE="$HOME/.local/share/applications/$APP.desktop"
ICON_FILE="$HOME/.local/share/icons/hicolor/128x128/apps/$APP.png"
AUTOSTART_FILE="$HOME/.config/autostart/$APP.desktop"
ICON_IN_APPIMAGE="usr/share/icons/hicolor/128x128/apps/$APP.png"
TMP_DIR=""

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31mError:\033[0m %s\n' "$*" >&2; exit 1; }

has_apt() { command -v apt-get >/dev/null 2>&1 && command -v dpkg >/dev/null 2>&1; }

# Matches the app however it was started (/usr/bin/..., bare name from the menu, or the
# binary inside a mounted AppImage) without matching anything that merely mentions the name.
stop_running_app() {
    if pgrep -xf "(.*/)?$APP" >/dev/null 2>&1; then
        say "Stopping the running app"
        pkill -xf "(.*/)?$APP" || true
        sleep 1
    fi
}

start_app() {
    say "Starting $NAME (look for the tray icon)"
    nohup "$1" >/dev/null 2>&1 &
    disown || true
}

uninstall() {
    stop_running_app
    if has_apt && dpkg -s "$APP" >/dev/null 2>&1; then
        say "Removing the $APP package"
        sudo apt-get remove -y "$APP"
    fi
    rm -f "$APPIMAGE_PATH" "$DESKTOP_FILE" "$ICON_FILE" "$AUTOSTART_FILE"
    say "Uninstalled. Your past verdicts are kept in ~/.local/share/dev.v6belung.now-playing-flagger/ (delete it to start fresh)."
}

# Prints the download URL of the latest release asset whose name ends in $1.
latest_asset_url() {
    local suffix="$1" json url
    json=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest") \
        || die "couldn't reach GitHub to find the latest release"
    url=$(grep -o '"browser_download_url": *"[^"]*"' <<<"$json" \
        | sed 's/.*"\(https[^"]*\)"/\1/' \
        | grep -E "_amd64\\$suffix\$" | head -n1) || true
    [ -n "$url" ] || die "the latest release has no Linux $suffix download yet"
    echo "$url"
}

download() {
    local url="$1" dest="$2"
    say "Downloading $(basename "$url")"
    curl -fL --progress-bar -o "$dest" "$url" || die "download failed"
}

install_deb() {
    local deb="$1"
    say "Installing the .deb (asks for your password)"
    # apt needs a path it can read as the _apt user; /tmp-style temp dirs can be private.
    local staged
    staged="$(mktemp -d /tmp/$APP-XXXXXX)"
    cp "$deb" "$staged/$APP.deb"
    chmod 755 "$staged"
    chmod 644 "$staged/$APP.deb"
    sudo apt-get install -y "$staged/$APP.deb"
    rm -rf "$staged"
    start_app "/usr/bin/$APP"
}

install_appimage() {
    local appimage="$1"
    say "Installing the AppImage to $APPIMAGE_PATH"
    mkdir -p "$BIN_DIR" "$(dirname "$DESKTOP_FILE")" "$(dirname "$ICON_FILE")"
    install -m 755 "$appimage" "$APPIMAGE_PATH"

    # The menu icon, pulled out of the AppImage itself so it always matches the version.
    local extract
    extract="$(mktemp -d)"
    if (cd "$extract" && "$APPIMAGE_PATH" --appimage-extract "$ICON_IN_APPIMAGE" >/dev/null 2>&1) \
        && [ -f "$extract/squashfs-root/$ICON_IN_APPIMAGE" ]; then
        cp "$extract/squashfs-root/$ICON_IN_APPIMAGE" "$ICON_FILE"
    fi
    rm -rf "$extract"

    cat >"$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=$NAME
GenericName=AI-Artist Flagger
Comment=Flags AI-generated artists in what you're playing on Spotify
Exec=$APPIMAGE_PATH
Icon=$APP
Categories=AudioVideo;Audio;Music;
Terminal=false
EOF
    command -v update-desktop-database >/dev/null 2>&1 \
        && update-desktop-database -q "$(dirname "$DESKTOP_FILE")" || true

    # Captured first: `ldconfig -p | grep -q` under pipefail reports failure when grep exits
    # early on a match.
    local libs
    libs="$(ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null || true)"
    if ! grep -q 'libfuse\.so\.2' <<<"$libs"; then
        printf '\033[33mNote:\033[0m AppImages need FUSE 2. If the app fails to start, install it\n' >&2
        printf '      (package "libfuse2", "fuse2" or "fuse-libs", depending on your distro).\n' >&2
    fi
    start_app "$APPIMAGE_PATH"
}

main() {
    local file=""
    case "${1:-}" in
        --uninstall) uninstall; return ;;
        --file) file="${2:-}"; [ -f "$file" ] || die "no such file: $file" ;;
        "") ;;
        *) die "unknown option: $1 (use --uninstall, --file PATH, or nothing)" ;;
    esac

    [ "$(uname -s)" = Linux ] || die "this installer is for Linux; Windows users can grab the .exe from the releases page"
    [ "$(uname -m)" = x86_64 ] || die "only x86_64 builds are published right now"
    command -v curl >/dev/null 2>&1 || [ -n "$file" ] || die "curl is required"

    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT

    stop_running_app

    if [ -n "$file" ]; then
        case "$file" in
            *.deb) install_deb "$file" ;;
            *.AppImage) install_appimage "$file" ;;
            *) die "--file expects a .deb or .AppImage" ;;
        esac
    elif has_apt; then
        # Assigned separately: a failure inside $(...) passed straight as an argument would
        # only exit the subshell, not the script.
        local url
        url="$(latest_asset_url .deb)"
        download "$url" "$TMP_DIR/$APP.deb"
        install_deb "$TMP_DIR/$APP.deb"
    else
        local url
        url="$(latest_asset_url .AppImage)"
        download "$url" "$TMP_DIR/$APP.AppImage"
        install_appimage "$TMP_DIR/$APP.AppImage"
    fi

    say "Done. Play something in the Spotify desktop app."
}

main "$@"
