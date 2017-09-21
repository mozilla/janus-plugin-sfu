PREFIX = /opt/janus/lib/janus/plugins
TARGET = target/release/libjanus_retproxy.so

$(TARGET):
	cargo build --release

test:
	cargo test

clean:
	cargo clean

install: $(TARGET) test
	mkdir -p $(DESTDIR)$(PREFIX)
	cp $< $(DESTDIR)$(PREFIX)

.PHONY: test clean install
