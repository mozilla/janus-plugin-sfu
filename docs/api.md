# Signalling API

**This documentation is in-progress and currently incomplete.**

The plugin exposes a signalling API for establishing connections and managing connection state.

[Janus has flexibility built-in][janus-transports] to control what transports can be used for signalling messages. We
expect consumers of this plugin to use WebSockets, but you can probably use whatever.

## Connection management and lifecycle

1. Signal your attachment to the Janus plugin. See the [Janus documentation][janus-transports] on how to attach to a
   plugin. This plugin's name is `janus.plugin.sfu`.

2. Create an RTC connection and perform session negotation.

3. Join a room. If you have a user ID, send your user ID; else obtain a user ID. Establish an initial set of subscriptions;
   subscriptions tell the server which data from other clients to send down your connection.

4. When done, close your connection, which will implicitly leave the room.

## Application protocol

Note that the signalling protocol is not strictly a request-response protocol. Messages you send may receive zero or
more related responses, and you should expect to receive signalling events that are not strictly responses to messages
you send.

All messages should be formatted as JSON objects.

### Messages you can send

#### Join

Joins a room and associates your connection with a user ID. No incoming or outgoing traffic will be relayed until you
join a room.

```json
{
    "kind": "join",
    "user_id": [none|unsigned integer ID],
    "role": [none|role descriptor object]
}
```

The first time you join a room, you should allow Janus to assign you a user ID; if you don't, you might overlap with
someone else's. For future connections, you should provide your user ID again. User IDs are used to identify the target
for subscriptions, so changing your user ID will make it impossible for people to subscribe to your audio.

You may pass a role descriptor object, joining as either a "subscriber":

```json
{
    "kind": "subscriber",
    "publisher_id": [unsigned integer user ID]
}
```

or a "publisher":

```json
{
    "kind": "publisher"
}
```

Passing either of these will set up an initial set of subscriptions; a "publisher" connection will be subscribed to the
data channel traffic of all other users in the room, and a "subscriber" connection will be subscribed to the audio and
video traffic of the user specified by the `publisher_id`.

Until Janus supports Unified Plan, the expectation is that most clients will have a single "publisher" connection that
carries all data channel traffic and outgoing audio/video, and many "subscriber" connections which carry incoming audio
and video streams from other clients.

### List

Lists all user IDs in a room, including your own, if you are in it.

```json
{
    "kind": "list"
}
```

### Subscribe

Subscribes to some kind of content, either from a specific user ID or from the whole room.

```json
{
    "kind": "subscribe",
    "publisher_id": [none|unsigned integer user ID],
    "content_kind": unsigned integer content kind
}
```

`content_kind` is currently a bit vector where 1 is audio, 2 is video, and 4 is data.


### Unsubscribe

Removes an existing subscription. Note that the spec for the subscription must currently be identical to when you
subscribed to it! For example, if you subscribe to (None, 1) and then you unsubscribe from ($UID, 1), you won't then get
content from everyone except $UID, and if you subscribe to ($UID, 255) and then you unsubscribe from ($UID, 1), you
won't get all content except audio from $UID.

```json
{
    "kind": "unsubscribe",
    "publisher_id": [none|unsigned integer user ID],
    "content_kind": unsigned integer content kind
}
```

[janus-transports]: https://janus.conf.meetecho.com/docs/rest.html
