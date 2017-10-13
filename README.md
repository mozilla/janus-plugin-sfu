# janus-plugin-sfu

[![Build Status](https://travis-ci.org/mquander/janus-plugin-sfu.svg?branch=master)](https://travis-ci.org/mquander/janus-plugin-sfu)

[Janus](https://janus.conf.meetecho.com/) [plugin](https://janus.conf.meetecho.com/docs/plugin_8h.html) to serve as a WebRTC SFU for game networking data.

In the future, this is likely to grow into a reverse proxy for [Reticulum](https://github.com/mozilla/reticulum), a kind of WebVR networking backend. But right now it's mostly just for being a simple, plug-and-play, star-topology SFU that you can use instead of being peer-to-peer.

[See here for API documentation on how to use the plugin.](docs/api.md)

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
