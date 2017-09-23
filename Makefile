PREFIX = /opt/janus/lib/janus/plugins
TARGET = target/release/libjanus_retproxy.so

install:
	cargo build --release
	cargo test --release
	mkdir -p $(DESTDIR)$(PREFIX)
	cp $(TARGET) $(DESTDIR)$(PREFIX)

.PHONY: install
