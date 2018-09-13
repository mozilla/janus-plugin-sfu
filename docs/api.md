# Signalling API

**This API is very WIP. So is this documentation.**

The plugin exposes a signalling API for establishing connections and managing connection state.

[Janus has flexibility built-in][janus-transports] to control what transports can be used for signalling messages. We
expect consumers of this plugin to use WebSockets, but you can probably use whatever.

## Connection management and lifecycle

1. Signal your attachment to the Janus plugin. See the [Janus documentation][janus-transports] on how to attach to a
   plugin. This plugin's name is `janus.plugin.sfu`.

2. Determine your user ID. This should be a unique ID that nobody else is likely to share. In the future, we will actually
   have authentication; as it stands just pick a big random ID and pray for no collisions.

3. Create an RTC connection.

4. Begin ICE negotiation.

5. If subscribing to data, establish data channels.

#### For connections that publish media

6. Add streams for the audio and video sources you're publishing.

7. Make an RTC offer and perform SDP negotiation.

8. Join a room. Establish a subscription to notifications or data, if desired.

#### For connections that subscribe to others' media

6. Join a room. Establish a subscription to notifications or data, if desired, as well as media from the user you want to subscribe to.

7. Take the JSEP offer which is returned and perform SDP negotiation by providing an answer.

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
    "room_id": room ID,
    "user_id": user ID,
    "subscribe": [none|subscription object]
}
```

If `subscription: {...}` is passed, you will synchronously configure an initial subscription to the traffic that you
want to get pushed through your connection. The format of the subscription should be identical to that in the
[subscribe](#subscribe) message, below.

The response will return the users on the server in the room you joined, as below, including yourself. If you `subscribe`d to a user's media, you will also get a JSEP offer you can use to get that user's RTP traffic.

```
{
    "success": true,
    "response": {
        "users": {room_alpha: ["123", "789"]}
    }
}
```

### Subscribe

Subscribes to some kind of traffic coming from the server.

```
{
    "kind": "subscribe",
    "notifications": [none|boolean],
    "data": [none|boolean],
    "media": [none|user ID]
}
```

If `notifications` is `true`, you will get websocket events corresponding to every time someone joins or leaves the server.

If `data` is `true`, you will get all data traffic from other users in your room, if you've joined a room.

If `media` is a user ID, the server will respond with a JSEP offer which you can use to establish a connection suitable to receive audio and video RTP data coming from that user ID.

### Block

Blocks another user. Blocks are bidirectional; the targeted user won't get your data, audio, or video, and you won't get
theirs. That user will get a `blocked` event letting them know.

```
{
    "kind": "block",
    "whom": [user ID]
}
```

Blocks persist between connections. If you block someone and refresh, they will still be blocked.

### Unblock

Unblock a user who you previously blocked. That user will get an `unblocked` event letting them know.

```
{
    "kind": "block",
    "whom": [user ID]
}
```

### Data

Sends a data payload string to all other users in the room, or to a specific user in the room. Useful for reliable
cross-client communication within a room without having to set up a WebRTC data channel.

```
{
    "kind": "data",
    "whom": [none|user ID]
    "body": string
}
```

[janus-transports]: https://janus.conf.meetecho.com/docs/rest.html
