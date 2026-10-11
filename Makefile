APP_BUNDLE := $(CURDIR)/dist/Varmint.app
APP_BIN := $(APP_BUNDLE)/Contents/MacOS/varmint
GUEST_DIR := $(CURDIR)/build/guest
PREFIX := $(CURDIR)/build/prefix

KERNEL ?= $(GUEST_DIR)/Image
INITRD ?= $(GUEST_DIR)/initrd
BASE_IMAGE ?= $(GUEST_DIR)/varmint-debian.raw.zst

.PHONY: app bundle dependencies guest-image run clean test

app: guest-image
	./scripts/build-app.sh --kernel "$(KERNEL)" --initrd "$(INITRD)" --base-image "$(BASE_IMAGE)"

bundle: guest-image
	./scripts/build-app.sh --skip-dependencies --kernel "$(KERNEL)" --initrd "$(INITRD)" --base-image "$(BASE_IMAGE)"

dependencies:
	./scripts/build-app.sh --dependencies-only

guest-image:
	./guest/build-image.sh

run: bundle
	"$(APP_BIN)" run --window

clean:
	rm -rf "$(CURDIR)/build" "$(CURDIR)/dist"

test:
	@test -f "$(PREFIX)/lib/libvirglrenderer.1.dylib" || \
		(echo "missing build dependencies; run: make dependencies" >&2; exit 2)
	PKG_CONFIG_PATH="$(PREFIX)/lib/pkgconfig:$${PKG_CONFIG_PATH:-}" \
		RUSTFLAGS="-L native=$(PREFIX)/lib $${RUSTFLAGS:-}" \
		DYLD_LIBRARY_PATH="$(PREFIX)/lib:$${DYLD_LIBRARY_PATH:-}" \
		cargo test
