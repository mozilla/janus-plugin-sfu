PREFIX = /opt/janus/lib/janus/plugins
TARGET = target/debug/libjanus_retproxy.so

install:
	cargo build
	cargo test
	mkdir -p $(DESTDIR)$(PREFIX)
	cp $(TARGET) $(DESTDIR)$(PREFIX)

.PHONY: install
