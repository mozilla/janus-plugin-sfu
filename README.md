# janus-plugin-sfu

[![Build Status](https://travis-ci.org/mozilla/janus-plugin-sfu.svg?branch=master)](https://travis-ci.org/mozilla/janus-plugin-sfu)

[Janus](https://janus.conf.meetecho.com/) [plugin](https://janus.conf.meetecho.com/docs/plugin_8h.html) to serve as a WebRTC Selective Forwarding Unit (SFU) for game networking data. It's the current backend for [Mozilla Hubs](https://github.com/mozilla/hubs).

In the future, this is likely to grow into a reverse proxy for [Reticulum](https://github.com/mozilla/reticulum), a kind of generalized, stateful, sharded WebVR networking backend. But right now it's mostly just for being a simple, plug-and-play, star-topology SFU that you can use instead of being peer-to-peer.

[See here for API documentation on how to communicate with the plugin.](docs/api.md)

We're hanging around in the [WebVR Slack](https://webvr-slack.herokuapp.com/) #social channel if you have any questions or want to chat. PRs and GitHub issues also welcome.

### How do I use this?

This is a plugin for Janus, so you'll need to install and run Janus first. The [installation instructions on GitHub](https://github.com/meetecho/janus-gateway#dependencies) are canonical. It's currently only compatible with recent master builds of Janus -- when Janus 0.4.0 is released, that will do.

This plugin should be compatible with any OS that can run Janus; that includes Linux, OS X, and Windows via WSL. If you're on Ubuntu, don't install the version from your package manager -- that one has no WebRTC data channel support, so it won't work. If that stresses you out, you can try running `scripts/setup-and-run-janus.sh`, which will compile and install Janus and its dependencies for you.

## Dependencies

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
