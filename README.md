# janus-plugin-sfu

[![Build Status](https://travis-ci.org/mquander/janus-plugin-sfu.svg?branch=master)](https://travis-ci.org/mquander/janus-plugin-sfu)

[Janus](https://janus.conf.meetecho.com/) [plugin](https://janus.conf.meetecho.com/docs/plugin_8h.html) to serve as a WebRTC SFU for game networking data.

In the future, this is likely to grow into a reverse proxy for [Reticulum](https://github.com/mozilla/reticulum), a kind of WebVR networking backend. But right now it's mostly just for being a simple, plug-and-play, star-topology SFU that you can use instead of being peer-to-peer.

[See here for API documentation on how to communicate with the plugin.](docs/api.md)

### How do I use this?

This is a plugin for Janus, so you'll need to install and run Janus first. The [installation instructions on GitHub](https://github.com/meetecho/janus-gateway#dependencies) are canonical.

If you're on Ubuntu, don't install the version from your package manager -- that one has no WebRTC data channel support, so it won't work. If that stresses you out, you can try running `stuff/setup-and-run-janus.sh`, which will compile and install Janus and its dependencies for you.

### Why shouldn't I just use janus_videoroom?

This one doesn't have all of the features of janus_videoroom yet, but it supports data channels. It's designed specifically for situations where video is not relevant but one needs multicasted audio and data.

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
