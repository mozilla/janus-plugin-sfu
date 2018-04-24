PREFIX = /opt/janus/lib/janus/plugins

install: release
	mkdir -p $(DESTDIR)$(PREFIX)
	cp target/release/libjanus_plugin_sfu.so $(DESTDIR)$(PREFIX)

release:
	RUSTFLAGS=-g cargo build --release
	cargo test --release

debug:
	cargo build
	cargo test

clean:
	cargo clean

.PHONY: clean install release debug
