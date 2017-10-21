var USER_ID = null; // set on initial connection
var ROOM_ID = 42;

const PEER_CONNECTION_CONFIG = {
    iceServers: [
        { urls: "stun:stun.l.google.com:19302" },
        { urls: "stun:global.stun.twilio.com:3478?transport=udp" }
    ]
};

function init() {
    var ws = new WebSocket("ws://localhost:8188", "janus-protocol");
    ws.addEventListener("open", () => {
        var session = new Minijanus.JanusSession(ws.send.bind(ws));
        ws.addEventListener("message", ev => handleMessage(session, ev));
        session.create().then(() => attachPublisher(session));
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
                    attachSubscriber(session, USER_ID, contents.user_id);
                }
                break;
            case "leave":
                // todo: tear down subscriber
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

function attachPublisher(session) {
    console.info("Attaching publisher for session: ", session);
    var conn = new RTCPeerConnection(PEER_CONNECTION_CONFIG);
    var handle = new Minijanus.JanusPluginHandle(session, conn);
    var publisher = handle.attach("janus.plugin.sfu").then(() => {
        var iceReady = handle.negotiateIce();
        var unreliableChannel = conn.createDataChannel("unreliable", { ordered: false, maxRetransmits: 0 });
        var reliableChannel = conn.createDataChannel("reliable", { ordered: true });
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
        return Promise.all([iceReady, localReady, remoteReady]).then(() => {
            console.info("Publisher connection established: ", handle);
            return handle;
        });
    });

    return publisher.then(handle => {
        return handle.sendMessage({ kind: "join", room_id: ROOM_ID }).then(reply => {
            var response = reply.plugindata.data.response;
            USER_ID = response.user_id;
            response.user_ids.forEach(otherId => {
                if (USER_ID !== otherId) {
                    attachSubscriber(session, otherId);
                }
            });
        });
    });
}

function attachSubscriber(session, otherId) {
    console.info("Attaching subscriber to " + otherId + " for session: ", session);
    var conn = new RTCPeerConnection(PEER_CONNECTION_CONFIG);
    var handle = new Minijanus.JanusPluginHandle(session, conn);
    conn.addEventListener("track", function(ev) {
        var receiverEl = document.createElement("audio");
        document.body.appendChild(receiverEl);
        receiverEl.srcObject = ev.streams[0];
        receiverEl.play();
    });

    var subscriber = handle.attach("janus.plugin.sfu").then(() => {
        var iceReady = handle.negotiateIce();
        var offerReady = conn.createOffer({ offerToReceiveAudio: true });
        var localReady = offerReady.then(conn.setLocalDescription.bind(conn));
        var remoteReady = offerReady
            .then(handle.sendJsep.bind(handle))
            .then(answer => conn.setRemoteDescription(answer.jsep));
        return Promise.all([iceReady, localReady, remoteReady]).then(() => {
            console.info("Subscriber connection established: ", handle);
            return handle;
        });
    });

    return subscriber.then(handle => {
        return handle.sendMessage({ kind: "join", room_id: ROOM_ID, user_id: USER_ID, subscription_specs: [
            { content_kind: 255, publisher_id: otherId }
        ]});
    });
}

init();
