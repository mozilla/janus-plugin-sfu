const params = new URLSearchParams(location.search.slice(1));
var USER_ID = Math.floor(Math.random() * (1000000001));
const roomId = params.get("room") != null ? parseInt(params.get("room")) : 42;

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

const status = document.getElementById("status");
function showStatus(message) {
  status.textContent = message;
}

function isError(signal) {
  var isPluginError =
      signal.plugindata &&
      signal.plugindata.data &&
      signal.plugindata.data.success === false;
  return isPluginError || Minijanus.JanusSession.prototype.isError(signal);
}

function connect(server) {
  document.getElementById("janusServer").value = server;
  showStatus(`Connecting to ${server}...`);
  var ws = new WebSocket(server, "janus-protocol");
  var session = c.session = new Minijanus.JanusSession(ws.send.bind(ws), { verbose: true });
  session.isError = isError;
  ws.addEventListener("message", ev => session.receive(JSON.parse(ev.data)));
  ws.addEventListener("open", _ => {
    session.create()
      .then(_ => attachPublisher(session))
      .then(x => { c.publisher = x; },
            err => console.error("Error attaching publisher: ", err));
  });
}

document.getElementById("micButton").addEventListener("click", _ => {
  var constraints = { audio: true };
  navigator.mediaDevices.getUserMedia(constraints)
    .then(m => m.getTracks().forEach(t => c.publisher.conn.addTrack(t, m)))
    .catch(e => console.error("Error requesting media: ", e));
});

document.getElementById("screenButton").addEventListener("click", _ => {
  var constraints = { video: { mediaSource: "screen" } };
  navigator.mediaDevices.getUserMedia(constraints)
    .then(m => m.getTracks().forEach(t => c.publisher.conn.addTrack(t, m)))
    .catch(e => console.error("Error requesting media: ", e));
});

function addUser(session, userId) {
  console.info("Adding user " + userId + ".");
  return attachSubscriber(session, userId)
    .then(x => { c.subscribers[userId] = x; },
          err => console.error("Error attaching subscriber: ", err));
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

let messages = [];

const messageCount = document.getElementById("messageCount");
function updateMessageCount() {
  messageCount.textContent = messages.length;
}

let firstMessageTime;
function storeMessage(data, reliable) {
  if (!firstMessageTime) {
    firstMessageTime = performance.now();
  }
  messages.push({
    time: performance.now() - firstMessageTime,
    reliable,
    message: JSON.parse(data)
  });
  updateMessageCount();
}

function storeReliableMessage(ev) {
  storeMessage(ev.data, true);
}

function storeUnreliableMessage(ev) {
  storeMessage(ev.data, false);
}

document.getElementById("saveButton").addEventListener("click", function saveToMessagesFile() {
  const file = new File([JSON.stringify(messages)], "messages.json", {type: "text/json"});
  saveAs(file);
});

document.getElementById("clearButton").addEventListener("click", function clearMessages() {
  messages = [];
  updateMessageCount();
});

function waitForEvent(name, handle) {
  return new Promise(resolve => handle.on(name, resolve));
}

function associate(conn, handle) {
  conn.addEventListener("icecandidate", ev => {
    handle.sendTrickle(ev.candidate || null).catch(e => console.error("Error trickling ICE: ", e));
  });
  conn.addEventListener("negotiationneeded", _ => {
    console.info("Sending new offer for handle: ", handle);
    var offer = conn.createOffer();
    var local = offer.then(o => conn.setLocalDescription(o));
    var remote = offer.then(j => handle.sendJsep(j)).then(r => conn.setRemoteDescription(r.jsep));
    Promise.all([local, remote]).catch(e => console.error("Error negotiating offer: ", e));
  });
  handle.on("event", ev => {
    if (ev.jsep && ev.jsep.type == "offer") {
      console.info("Accepting new offer for handle: ", handle);
      var answer = conn.setRemoteDescription(ev.jsep).then(_ => conn.createAnswer());
      var local = answer.then(a => conn.setLocalDescription(a));
      var remote = answer.then(j => handle.sendJsep(j));
      Promise.all([local, remote]).catch(e => console.error("Error negotiating answer: ", e));
    }
  });
}

function attachPublisher(session) {
  console.info("Attaching publisher for session: ", session);
  var conn = new RTCPeerConnection(PEER_CONNECTION_CONFIG);
  var handle = new Minijanus.JanusPluginHandle(session);
  associate(conn, handle);

  // Handle all of the join and leave events.
  handle.on("event", ev => {
    var data = ev.plugindata.data;
    if (data.event == "join" && data.room_id == roomId) {
      this.addUser(session, data.user_id);
    } else if (data.event == "leave" && data.room_id == roomId) {
      this.removeUser(session, data.user_id);
    }
  });

  return handle.attach("janus.plugin.sfu").then(() => {
    showStatus(`Connecting WebRTC...`);
    const reliableChannel = conn.createDataChannel("reliable", { ordered: true });
    reliableChannel.addEventListener("message", storeReliableMessage);
    const unreliableChannel = conn.createDataChannel("unreliable", { ordered: false, maxRetransmits: 0 });
    unreliableChannel.addEventListener("message", storeUnreliableMessage);
    return waitForEvent("webrtcup", handle)
      .then(_ => {
        showStatus(`Joining room ${roomId}...`);
        return handle.sendMessage({ kind: "join", room_id: roomId, user_id: USER_ID, subscribe: { notifications: true, data: true }});
      })
      .then(reply => {
        showStatus(`Subscribing to others in room ${roomId}`);
        var occupants = reply.plugindata.data.response.users[roomId] || [];
        return Promise.all(occupants.map(userId => addUser(session, userId)));
      })
      .then(_ => { return { handle, conn, reliableChannel, unreliableChannel }; });
  });
}

function attachSubscriber(session, otherId) {
  console.info("Attaching subscriber to " + otherId + " for session: ", session);
  var conn = new RTCPeerConnection(PEER_CONNECTION_CONFIG);
  var handle = new Minijanus.JanusPluginHandle(session);
  associate(conn, handle);

  conn.addEventListener("track", ev => {
    console.info("Attaching " + ev.track.kind + " track from " + otherId + " for session: ", session);
    var mediaEl = document.createElement(ev.track.kind);
    document.body.appendChild(mediaEl);
    mediaEl.controls = true;
    mediaEl.srcObject = ev.streams[0];
    mediaEl.play();
  });

  return handle.attach("janus.plugin.sfu")
    .then(_ => handle.sendMessage({ kind: "join", room_id: roomId, user_id: USER_ID, subscribe: { media: otherId }}))
    .then(_ => waitForEvent("webrtcup", handle))
    .then(_ => { return { handle: handle, conn: conn }; });
}

connect(params.get("janus") || `ws://localhost:8188`);
