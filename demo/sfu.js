var USER_ID = Math.floor(Math.random() * (1000000001));
var ROOM_ID = 42;

const PEER_CONNECTION_CONFIG = {
    iceServers: [
        { urls: "stun:stun.l.google.com:19302" },
        { urls: "stun:global.stun.twilio.com:3478?transport=udp" }
    ]
};

// global helper for interactive use
var c = {
    session: null,
    publisher: null,
    subscribers: {}
};

function init() {
    var ws = new WebSocket("ws://localhost:8188", "janus-protocol");
    ws.addEventListener("open", () => {
        var session = c.session = new Minijanus.JanusSession(ws.send.bind(ws));
        ws.addEventListener("message", ev => handleMessage(session, ev));
        session.create().then(() => attachPublisher(session)).then(x => {
            c.publisher = x;
        });
    });
}

function handleMessage(session, ev) {
    var data = JSON.parse(ev.data);
    session.receive(data);
    if (data.janus === "event") {
        if (data.plugindata && data.plugindata.data) {
            var contents = data.plugindata.data;
            switch (contents.event) {
            case "join":
                if (USER_ID !== contents.user_id) {
                    addUser(session, contents.user_id);
                }
                break;
            case "leave":
                removeUser(session, contents.user_id);
                break;
            case undefined:
                // a non-plugin event
                break;
            default:
                console.error("Unknown event received: ", data.plugindata.data);
                break;
            }
        }
    }
}

function negotiateIce(conn, handle) {
    return new Promise((resolve, reject) => {
        conn.addEventListener("icecandidate", ev => {
            handle.sendTrickle(ev.candidate || null).then(() => {
                if (!ev.candidate) { // this was the last candidate on our end and now they received it
                    resolve();
                }
            });
        });
    });
};

function addUser(session, userId) {
    console.info("Adding user " + userId + ".");
    attachSubscriber(session, userId).then(x => {
        c.subscribers[userId] = x;
    });
}

function removeUser(session, userId) {
    console.info("Removing user " + userId + ".");
    var subscriber = c.subscribers[userId];
    if (subscriber != null) {
        subscriber.handle.detach();
        subscriber.conn.close();
        delete c.subscribers[userId];
    }
}

function attachPublisher(session) {
    console.info("Attaching publisher for session: ", session);
    var conn = new RTCPeerConnection(PEER_CONNECTION_CONFIG);
    var handle = new Minijanus.JanusPluginHandle(session);
    return handle.attach("janus.plugin.sfu").then(() => {
        var iceReady = negotiateIce(conn, handle);
        var channel = conn.createDataChannel("reliable", { ordered: true });
        channel.addEventListener("message", ev => console.info("Message received on channel: ", ev));
        var mediaReady = navigator.mediaDevices.getUserMedia({ audio: true });
        var offerReady = mediaReady
            .then(media => {
                conn.addStream(media);
                return conn.createOffer({ audio: true });
            }, () => conn.createOffer());
        var localReady = offerReady.then(conn.setLocalDescription.bind(conn));
        var remoteReady = offerReady
            .then(handle.sendJsep.bind(handle))
            .then(answer => conn.setRemoteDescription(answer.jsep));
        var connectionReady = Promise.all([iceReady, localReady, remoteReady]);
        return connectionReady
            .then(() => handle.sendMessage({ kind: "join", room_id: ROOM_ID, user_id: USER_ID, notify: true }))
            .then(reply => {
                var response = reply.plugindata.data.response;
                response.user_ids.forEach(otherId => {
                    if (USER_ID !== otherId) {
                        addUser(session, otherId);
                    }
                });
                return { handle: handle, conn: conn, channel: channel };
            });
    });
}

function attachSubscriber(session, otherId) {
    console.info("Attaching subscriber to " + otherId + " for session: ", session);
    var conn = new RTCPeerConnection(PEER_CONNECTION_CONFIG);
    conn.addEventListener("track", function(ev) {
        var receiverEl = document.createElement("audio");
        document.body.appendChild(receiverEl);
        receiverEl.srcObject = ev.streams[0];
        receiverEl.play();
    });

    var handle = new Minijanus.JanusPluginHandle(session);
    return handle.attach("janus.plugin.sfu")
        .then(() => {
            var iceReady = negotiateIce(conn, handle);
            var offerReady = conn.createOffer({ offerToReceiveAudio: true });
            var localReady = offerReady.then(conn.setLocalDescription.bind(conn));
            var remoteReady = offerReady
                .then(handle.sendJsep.bind(handle))
                .then(answer => conn.setRemoteDescription(answer.jsep));
            var connectionReady = Promise.all([iceReady, localReady, remoteReady]);
            return connectionReady.then(() => {
                return handle.sendMessage({ kind: "join", room_id: ROOM_ID, user_id: USER_ID, subscription_specs: [
                    { content_kind: 255, publisher_id: otherId }
                ]});

            }).then(reply => {
                return { handle: handle, conn: conn };
            });

        });
}

init();
