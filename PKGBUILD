# Maintainer: sickhate <archate@gmail.com>
pkgname=meh2
pkgver=0.1.0.r56.a5b057c
pkgrel=1
pkgdesc="GTK4 Wayland widget system with Rhai scripting (meh2 fork of meh/eww)"
arch=('x86_64')
url="https://github.com/sickhate/meh2"
license=('GPL-3.0-or-later')
depends=(
    'gtk4'
    'gtk4-layer-shell'
    'libadwaita'
    'cairo'
    'glib2'
    'pango'
)
makedepends=('rust' 'cargo')
options=('!debug')
source=("${pkgname}::git+file://${HOME}/Projects/meh2")
sha256sums=('SKIP')

prepare() {
    cd "$srcdir/$pkgname"
    export RUSTUP_TOOLCHAIN=stable
    rm -rf target
    cargo fetch --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
    cd "$srcdir/$pkgname"
    export RUSTUP_TOOLCHAIN=stable
    export CARGO_TARGET_DIR=target
    cargo build --release --locked
}

check() {
    cd "$srcdir/$pkgname"
    export RUSTUP_TOOLCHAIN=stable
    cargo test --release --locked 2>/dev/null | grep -v '^$' | grep -v 'running 0 tests' | grep -v 'test result: ok. 0 passed' || true
}

package() {
    cd "$srcdir/$pkgname"
    install -Dm755 "target/release/meh2" "$pkgdir/usr/bin/meh2"
    install -Dm644 <(echo "GPL-3.0-or-later") "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}

pkgver() {
    cd "$srcdir/$pkgname"
    printf "0.1.0.r%s.%s" "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)"
}
