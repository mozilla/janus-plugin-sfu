PREFIX = /opt/janus/lib/janus/plugins
TARGET = target/release/libjanus_plugin_sfu.so

install:
	cargo build --release
	cargo test
	mkdir -p $(DESTDIR)$(PREFIX)
	cp $(TARGET) $(DESTDIR)$(PREFIX)

.PHONY: install
