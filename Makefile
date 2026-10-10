CARGO ?= cargo
PREFIX ?= $(HOME)/.local/bin

.PHONY: all build rm

all: build

build:
	$(CARGO) build --release
	mkdir -p $(PREFIX)
	cp target/release/xvpn $(PREFIX)/xvpn
	chmod +x $(PREFIX)/xvpn

rm:
	rm -f $(PREFIX)/xvpn
