# Signalling API

**This API is very WIP. So is this documentation.**

The plugin exposes a signalling API for establishing connections and managing connection state.

[Janus has flexibility built-in][janus-transports] to control what transports can be used for signalling messages. We
expect consumers of this plugin to use WebSockets, but you can probably use whatever.

## Connection management and lifecycle

1. Signal your attachment to the Janus plugin. See the [Janus documentation][janus-transports] on how to attach to a
   plugin. This plugin's name is `janus.plugin.sfu`.

2. Create an RTC connection and perform session negotation.

3. Determine your user ID. This should be a unique ID that nobody else is likely to share. In the future, we will actually
   have authentication; as it stands just pick a big random ID and pray for no collisions. I'm serious.

4. Join a room. Establish an initial set of subscriptions; subscriptions tell the server which data from other clients
   to send down your connection.

5. When done, close your connection, which will implicitly leave the room.

## Application protocol

Note that the signalling protocol is not strictly a request-response protocol. Messages you send may receive zero or
more related responses, and you should expect to receive signalling events that are not strictly responses to messages
you send.

All messages should be formatted as JSON objects.

### Messages you can send

#### Join

Joins a room and associates your connection with a user ID. No incoming or outgoing traffic will be relayed until you
join a room. You can only join one room with any connection.

```
{
    "kind": "join",
    "room_id": unsigned integer ID
    "user_id": unsigned integer ID,
    "notify": [none|boolean],
    "subscription_specs": [none|array of spec objects]
}
```

If `notify: true` is passed, you will receive notifications from Janus for this handle when things relevant to your
interest occur in the room; for example, if someone joins or leaves. If you create multiple connections, you probably
don't want those notifications on every connection.

If `subscription_specs: [...]` is passed, you will synchronously configure initial subscriptions to the audio and video
for other users in the room as per the specs. The format of the objects in the `subscription_specs` array should be
identical to those in the [subscribe](#subscribe) message, below.

The response will return all users other than yourself who are in your current room.

```
{
    "success": true,
    "user_ids": [123, 789]
}
```

### List rooms

Lists all rooms that anyone is connected to, including your own.

```
{
    "kind": "listrooms"
}
```

```
{
    "success": true,
    "room_ids": [1, 5, 42]
}
```

### List users

Lists all users in the given room, including you, if you're in it.

```
{
    "kind": "listusers"
    "room_id": unsigned integer room ID
}
```

```
{
    "success": true,
    "user_ids": [123, 456, 789]
}
```

### Subscribe

Subscribes to some kind of content, either from a specific user ID or from the whole room.

```
{
    "kind": "subscribe",
    "specs": [spec]
}
```

where each spec describes a subscription:

```
{
    "publisher_id": unsigned integer user ID,
    "content_kind": unsigned integer content kind
}
```

`content_kind` is currently a bit vector where 1 is audio and 2 is video.

Until Janus supports Unified Plan, the expectation is that most clients will have a single "publisher" connection that
only sends audio and video and doesn't receive any, and many "subscriber" connections which subscribe to incoming audio
and video streams from other clients.

### Unsubscribe

Removes some existing subscription specs. Note that the spec for the subscription must currently be identical to when you
subscribed to it! For example, if you subscribe to ($UID, 255) and then you unsubscribe from ($UID, 1), you
won't get all content except audio from $UID.

```
{
    "kind": "unsubscribe",
    "specs": [spec]
}
```

where each spec describes a subscription:

```
{
    "publisher_id": unsigned integer user ID,
    "content_kind": unsigned integer content kind
}
```

[janus-transports]: https://janus.conf.meetecho.com/docs/rest.html
