APP_NAME := blurred
BIN_NAME := blurred
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
DIST_DIR := dist
RELEASE_BIN := target/release/$(BIN_NAME)
HOST_OS := $(shell uname -s)

.PHONY: run check build release clean dist package package-host package-macos package-linux package-windows version

run:
	cargo run

check:
	cargo check

build:
	cargo build

release:
	cargo build --release

clean:
	cargo clean
	rm -rf $(DIST_DIR)

dist:
	mkdir -p $(DIST_DIR)

version:
	@echo $(VERSION)

package: release package-host

package-host:
ifeq ($(HOST_OS),Darwin)
	$(MAKE) package-macos
else ifeq ($(HOST_OS),Linux)
	$(MAKE) package-linux
else
	$(MAKE) package-windows
endif

package-macos: release dist
	rm -rf "$(DIST_DIR)/$(APP_NAME).app" "$(DIST_DIR)/$(APP_NAME)-macos-v$(VERSION).zip"
	mkdir -p "$(DIST_DIR)/$(APP_NAME).app/Contents/MacOS"
	mkdir -p "$(DIST_DIR)/$(APP_NAME).app/Contents/Resources"
	cp "$(RELEASE_BIN)" "$(DIST_DIR)/$(APP_NAME).app/Contents/MacOS/$(BIN_NAME)"
	cp "logo.png" "$(DIST_DIR)/$(APP_NAME).app/Contents/Resources/logo.png"
	printf '%s\n' \
		'<?xml version="1.0" encoding="UTF-8"?>' \
		'<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
		'<plist version="1.0">' \
		'<dict>' \
		'  <key>CFBundleDevelopmentRegion</key><string>en</string>' \
		'  <key>CFBundleExecutable</key><string>$(BIN_NAME)</string>' \
		'  <key>CFBundleIdentifier</key><string>com.byteface.blurred</string>' \
		'  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>' \
		'  <key>CFBundleName</key><string>$(APP_NAME)</string>' \
		'  <key>CFBundleDisplayName</key><string>$(APP_NAME)</string>' \
		'  <key>CFBundleIconFile</key><string>logo.png</string>' \
		'  <key>CFBundlePackageType</key><string>APPL</string>' \
		'  <key>CFBundleShortVersionString</key><string>$(VERSION)</string>' \
		'  <key>CFBundleVersion</key><string>$(VERSION)</string>' \
		'  <key>CFBundleDocumentTypes</key>' \
		'  <array>' \
		'    <dict>' \
		'      <key>CFBundleTypeName</key><string>Text Documents</string>' \
		'      <key>CFBundleTypeRole</key><string>Viewer</string>' \
		'      <key>LSItemContentTypes</key>' \
		'      <array>' \
		'        <string>public.plain-text</string>' \
		'        <string>public.rtf</string>' \
		'        <string>net.daringfireball.markdown</string>' \
		'      </array>' \
		'    </dict>' \
		'  </array>' \
		'  <key>LSMinimumSystemVersion</key><string>10.13</string>' \
		'</dict>' \
		'</plist>' > "$(DIST_DIR)/$(APP_NAME).app/Contents/Info.plist"
	ditto -c -k --sequesterRsrc --keepParent "$(DIST_DIR)/$(APP_NAME).app" "$(DIST_DIR)/$(APP_NAME)-macos-v$(VERSION).zip"

package-linux: release dist
	rm -f "$(DIST_DIR)/$(APP_NAME)-linux-v$(VERSION).tar.gz"
	tar -czf "$(DIST_DIR)/$(APP_NAME)-linux-v$(VERSION).tar.gz" -C target/release $(BIN_NAME)

package-windows: release dist
	rm -f "$(DIST_DIR)/$(APP_NAME)-windows-v$(VERSION).zip"
	zip -j "$(DIST_DIR)/$(APP_NAME)-windows-v$(VERSION).zip" "$(RELEASE_BIN).exe"
