#!/bin/bash
# Anchor Compositor — One-click installer
# Usage: curl -sSL https://raw.githubusercontent.com/ai-pivot/Anchor/master/scripts/install.sh | bash
# Or:    ./scripts/install.sh [--uninstall]

set -e

REPO="https://github.com/ai-pivot/Anchor.git"
INSTALL_PREFIX="${INSTALL_PREFIX:-/usr/local}"
UNINSTALL=false

# Parse args
for arg in "$@"; do
    case "$arg" in
        --uninstall|-u) UNINSTALL=true ;;
        --prefix=*) INSTALL_PREFIX="${arg#--prefix=}" ;;
        --help|-h)
            echo "Usage: $0 [--uninstall] [--prefix=/usr/local]"
            echo ""
            echo "  --uninstall    Remove Anchor compositor"
            echo "  --prefix=DIR   Install prefix (default: /usr/local)"
            exit 0
            ;;
    esac
done

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()  { echo -e "${BLUE}[INFO]${NC} $*"; }
ok()    { echo -e "${GREEN}[OK]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
err()   { echo -e "${RED}[ERROR]${NC} $*" >&2; exit 1; }

# ─── Uninstall ───
if $UNINSTALL; then
    info "Uninstalling Anchor compositor..."
    sudo rm -f "$INSTALL_PREFIX/bin/anchor"
    sudo rm -f "$INSTALL_PREFIX/bin/anchor-session"
    sudo rm -f "/usr/share/wayland-sessions/anchor.desktop"
    sudo rm -rf "/usr/share/doc/anchor"
    ok "Anchor uninstalled."
    exit 0
fi

# ─── Preflight checks ───
info "Anchor Compositor Installer"
echo ""

# Check if already installed
if command -v anchor &>/dev/null; then
    warn "Anchor is already installed: $(command -v anchor)"
    read -p "Reinstall? [y/N] " -r
    [[ ! $REPLY =~ ^[Yy]$ ]] && exit 0
fi

# Check Rust
if ! command -v cargo &>/dev/null; then
    info "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi
ok "Rust: $(rustc --version)"

# ─── Detect distro and install deps ───
detect_distro() {
    if [ -f /etc/arch-release ]; then echo "arch"
    elif [ -f /etc/fedora-release ]; then echo "fedora"
    elif [ -f /etc/debian_version ]; then echo "debian"
    elif [ -f /etc/os-release ]; then
        . /etc/os-release
        case "$ID" in
            ubuntu|debian|pop|linuxmint) echo "debian" ;;
            fedora) echo "fedora" ;;
            opensuse*) echo "opensuse" ;;
            *) echo "unknown" ;;
        esac
    else echo "unknown"
    fi
}

DISTRO=$(detect_distro)
info "Detected distro: $DISTRO"

install_deps() {
    case "$DISTRO" in
        arch)
            info "Installing build dependencies (Arch)..."
            sudo pacman -S --needed --noconfirm \
                base-devel rust pam libinput libxkbcommon \
                mesa systemd-libs wayland clang pkgconf git
            ;;
        fedora)
            info "Installing build dependencies (Fedora)..."
            sudo dnf install -y \
                gcc clang make pkg-config \
                pam-devel libinput-devel libxkbcommon-devel \
                mesa-libEGL-devel mesa-libgbm-devel \
                systemd-devel wayland-devel git \
                rust cargo
            ;;
        debian)
            info "Installing build dependencies (Debian/Ubuntu)..."
            sudo apt-get update
            sudo apt-get install -y \
                build-essential clang pkg-config \
                libpam0g-dev libinput-dev libxkbcommon-dev \
                libegl-dev libgbm-dev libdrm-dev \
                libsystemd-dev libwayland-dev git \
                rustc cargo
            ;;
        opensuse)
            info "Installing build dependencies (openSUSE)..."
            sudo zypper install -y \
                gcc clang pkg-config \
                pam-devel libinput-devel libxkbcommon-devel \
                Mesa-libEGL-devel Mesa-libgbm-devel \
                systemd-devel wayland-devel git \
                rust cargo
            ;;
        *)
            warn "Unknown distro. Please install these deps manually:"
            echo "  Rust (rustup.rs), PAM, libinput, libxkbcommon,"
            echo "  mesa/EGL/GBM, systemd (libudev), wayland, clang, pkg-config"
            read -p "Press Enter after installing deps, or Ctrl+C to abort..."
            ;;
    esac
}

# Check if we need deps
if ! pkg-config --exists libinput libxkbcommon wayland-client 2>/dev/null; then
    install_deps
else
    ok "Build dependencies satisfied."
fi

# ─── Clone and build ───
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

info "Cloning Anchor..."
git clone --depth 1 "$REPO" "$TMPDIR/Anchor"
cd "$TMPDIR/Anchor"

info "Building Anchor (this may take a few minutes)..."
cargo build --release
ok "Build complete."

# ─── Install ───
info "Installing to $INSTALL_PREFIX..."

sudo install -Dm755 "target/release/anchor" "$INSTALL_PREFIX/bin/anchor"
sudo install -Dm755 "scripts/anchor-session" "$INSTALL_PREFIX/bin/anchor-session"
sudo install -Dm644 "scripts/anchor.desktop" "/usr/share/wayland-sessions/anchor.desktop"
sudo install -Dm644 "config.toml" "/usr/share/doc/anchor/config.toml.example"

ok "Files installed."

# ─── Post-install ───
echo ""
ok "Anchor compositor installed successfully!"
echo ""
echo -e "${YELLOW}Post-install steps:${NC}"
echo ""
echo "  1. Copy example config (optional):"
echo "     mkdir -p ~/.config/anchor"
echo "     cp /usr/share/doc/anchor/config.toml.example ~/.config/anchor/config.toml"
echo ""
echo "  2. Select 'Anchor' from your display manager (GDM/SDDM/LightDM)"
echo ""
echo "  3. NVIDIA users: ensure nvidia-drm.modeset=1 kernel parameter is set"
echo ""
echo -e "  Uninstall: ${BLUE}curl -sSL $REPO/raw/master/scripts/install.sh | bash -s -- --uninstall${NC}"
echo ""
