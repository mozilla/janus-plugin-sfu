# janus-plugin-sfu

[![Build Status](https://travis-ci.org/mozilla/janus-plugin-sfu.svg?branch=master)](https://travis-ci.org/mozilla/janus-plugin-sfu)

[Janus](https://janus.conf.meetecho.com/) [plugin](https://janus.conf.meetecho.com/docs/plugin_8h.html) to serve as a WebRTC Selective Forwarding Unit (SFU) for game networking data. It's the current backend for [Mozilla Hubs](https://github.com/mozilla/hubs). It's mostly just for being a simple, plug-and-play, star-topology SFU that you can use instead of being peer-to-peer.

[See here for API documentation on how to communicate with the plugin.](docs/api.md)

PRs and GitHub issues are welcome.

### How do I use this?

This is a plugin for Janus, so you'll need to install and run Janus first. The [installation instructions on GitHub](https://github.com/meetecho/janus-gateway#dependencies) are canonical. It's compatible with Janus version 0.7.3 and later, although sometimes Janus makes changes that break plugins.

This plugin should be compatible with any OS that can run Janus; that includes Linux, OS X, and Windows via WSL. If you use a version from a package manager, you might want to check to make sure it has data channel support, which is a compile-time option. (Debian and Ubuntu have it.)

## Dependencies

These are the native dependencies necessary for building the Rust plugin. For Janus's dependencies, consult its documentation.
```
$ sudo apt install libglib2.0-dev libjansson-dev
```

## Building

```
$ cargo build [--release]
```

## Testing

```
$ cargo test
```

## Installing

Install the library output by the build process (e.g. ./target/release/libjanus_plugin_sfu.so) into the Janus plugins
directory (e.g. /usr/lib/janus/plugins). Restart Janus to activate.

## Configuration and usage

The plugin accepts a configuration file in the Janus configuration directory named `janus.plugin.sfu.cfg` containing key/value pairs in INI format. An example configuration file is provided as `janus.plugin.sfu.cfg.example`.

You can test your install by pointing a browser at the `tiny.html` client provided in the `client` directory. If you open two browser windows, you should be able to share your microphone, share your screen, and send data channel messages in one, and see the results in the other.
