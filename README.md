# janus-retproxy

[![Build Status](https://travis-ci.org/mquander/janus-retproxy.svg?branch=master)](https://travis-ci.org/mquander/janus-retproxy)

[Janus](https://janus.conf.meetecho.com/) plugin to serve as a reverse proxy for [Reticulum](https://github.com/mozilla/reticulum), a kind of WebVR networking backend.

## Building

```
$ cargo build
```

## Installing

Install the library output by the build process (e.g. ./target/debug/libjanus_retproxy.so) into the Janus plugins
directory (e.g. /usr/lib/janus/plugins). Restart Janus to activate.
