CARGO ?= cargo
PREFIX ?= $(HOME)/.local/bin
STATE_DIR ?= $(HOME)/.local/state/xvpn
LOG_FILES := $(STATE_DIR)/xray.log $(STATE_DIR)/sing-box.log \
	$(STATE_DIR)/supervisor.log $(STATE_DIR)/diagnose.log

.PHONY: all build clean-logs diagnose rm

all: build

build: clean-logs
	$(CARGO) build --release
	mkdir -p $(PREFIX)
	cp target/release/xvpn $(PREFIX)/.xvpn.tmp
	chmod +x $(PREFIX)/.xvpn.tmp
	mv $(PREFIX)/.xvpn.tmp $(PREFIX)/xvpn
	$(PREFIX)/xvpn agent restart

clean-logs:
	rm -f $(LOG_FILES)

rm: clean-logs
	rm -f $(PREFIX)/xvpn
