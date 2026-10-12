DIST := $(CURDIR)/dist/varmint
GUEST_DIR := $(CURDIR)/build/guest
PREFIX := $(CURDIR)/build/prefix

KERNEL ?= $(GUEST_DIR)/Image
INITRD ?= $(GUEST_DIR)/initrd
BASE_IMAGE ?= $(GUEST_DIR)/varmint-debian.raw.zst
INSTALL_PREFIX ?= $(HOME)/.local

.PHONY: build bundle dependencies guest-image install pkg run clean test

build: guest-image
	./scripts/build.sh --kernel "$(KERNEL)" --initrd "$(INITRD)" --base-image "$(BASE_IMAGE)"

bundle: guest-image
	./scripts/build.sh --skip-dependencies --kernel "$(KERNEL)" --initrd "$(INITRD)" --base-image "$(BASE_IMAGE)"

dependencies:
	./scripts/build.sh --dependencies-only

guest-image:
	./guest/build-image.sh

install: bundle
	./scripts/install.sh "$(INSTALL_PREFIX)"

# Installer for /usr/local/bin/varmint and its runtime in /usr/local/share/varmint.
pkg: bundle
	pkgbuild --root "$(DIST)" --install-location /usr/local --identifier dev.varmint.cli \
		--version "$$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)" "$(CURDIR)/dist/varmint.pkg"

run: bundle
	"$(DIST)/bin/varmint" run --window

clean:
	rm -rf "$(CURDIR)/build" "$(CURDIR)/dist"

test:
	@test -f "$(PREFIX)/lib/libvirglrenderer.1.dylib" || \
		(echo "missing build dependencies; run: make dependencies" >&2; exit 2)
	PKG_CONFIG_PATH="$(PREFIX)/lib/pkgconfig:$${PKG_CONFIG_PATH:-}" \
		RUSTFLAGS="-L native=$(PREFIX)/lib $${RUSTFLAGS:-}" \
		DYLD_LIBRARY_PATH="$(PREFIX)/lib:$${DYLD_LIBRARY_PATH:-}" \
		cargo test
