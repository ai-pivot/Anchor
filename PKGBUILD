# Maintainer: Anchor Contributors
pkgname=anchor-wl
pkgver=0.5.0
pkgrel=1
pkgdesc="Minimal tiling Wayland compositor with NVIDIA/AMD/Intel GPU support"
arch=('x86_64')
url="https://github.com/ai-pivot/Anchor"
license=('MIT')
depends=(
  'gcc-libs'
  'glibc'
  'libinput'
  'libxkbcommon'
  'mesa'
  'pam'
  'systemd-libs'
  'wayland'
)
makedepends=(
  'rust'
  'clang'
  'pkgconf'
  'git'
)
optdepends=(
  'foot: default terminal emulator'
  'fcitx5: input method framework'
  'gnome-keyring: secret storage for browsers'
)
provides=('anchor')
conflicts=('anchor')
source=("$pkgname-$pkgver.tar.gz::https://github.com/ai-pivot/Anchor/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')  # Update with actual hash on release

build() {
  cd "Anchor-$pkgver"
  cargo build --release --locked
}

package() {
  cd "Anchor-$pkgver"

  # Binary
  install -Dm755 "target/release/anchor" "$pkgdir/usr/bin/anchor"

  # Session wrapper script
  install -Dm755 "scripts/anchor-session" "$pkgdir/usr/bin/anchor-session"

  # Wayland session desktop file
  install -Dm644 "scripts/anchor.desktop" "$pkgdir/usr/share/wayland-sessions/anchor.desktop"

  # Example config
  install -Dm644 "config.toml" "$pkgdir/usr/share/doc/anchor/config.toml.example"

  # License
  install -Dm644 /dev/stdin "$pkgdir/usr/share/licenses/$pkgname/LICENSE" <<EOF
MIT License — see https://github.com/ai-pivot/Anchor for full text.
EOF
}
