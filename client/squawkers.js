Minijanus.verbose = true;

const e = React.createElement;
const peerConfig = { iceServers: [{ urls: "stun:stun.l.google.com:19302" }] };

class Squawker {
  constructor(audioFile, videoFile, dataFile, userId) {
    this.audioFile = audioFile;
    this.videoFile = videoFile;
    this.dataFile = dataFile;
    this.userId = userId;

    this.audioUrl = audioFile != null ? URL.createObjectURL(audioFile) : null;
    this.videoUrl = videoFile != null ? URL.createObjectURL(videoFile) : null;
  }

  static negotiateIce(conn, handle) {
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
}

class SquawkerItem extends React.Component {

  componentWillMount() {
    this.attachPublisher(this.props.session);
  }

  captureStream(el) {
    if (el.captureStream) {
      return el.captureStream();
    } else if (el.mozCaptureStream) {
      return el.mozCaptureStream();
    } else {
      throw new Error("Neither captureStream or mozCaptureStream is available in your browser.");
    }
  }

  getAudioStream() {
    if (!this.props.squawker.audioFile) { return null; }
    return this.captureStream(this.audioEl);
  }

  getVideoStream() {
    if (!this.props.squawker.videoFile) { return null; }
    return this.captureStream(this.videoEl);
  }

  attachPublisher(session) {
    console.info("Attaching publisher for squawker: ", this.props.squawker.url);
    const conn = new RTCPeerConnection(peerConfig);
    const handle = new Minijanus.JanusPluginHandle(session);
    this.setState({ conn: conn });
    return handle.attach("janus.plugin.sfu").then(() => {
      this.setState({ handle: handle });
      var iceReady = Squawker.negotiateIce(conn, handle);

      const audioStream = this.getAudioStream();
      if (audioStream) { conn.addStream(audioStream); }

      const videoStream = this.getVideoStream();
      if (videoStream) { conn.addStream(videoStream); }

      const reliableChannel = conn.createDataChannel("reliable", { ordered: true });
      const unreliableChannel = conn.createDataChannel("unreliable", { ordered: false, maxRetransmits: 0 });

      var offerReady = conn.createOffer({ offerToReceiveAudio: 0, offerToReceiveVideo: 0 });
      var localReady = offerReady.then(conn.setLocalDescription.bind(conn));
      var remoteReady = offerReady
          .then(handle.sendJsep.bind(handle))
          .then(answer => conn.setRemoteDescription(answer.jsep));
      var connectionReady = Promise.all([iceReady, localReady, remoteReady]);
      return connectionReady.then(() => handle.sendMessage({
        kind: "join",
        room_id: this.props.roomId,
        user_id: this.props.squawker.userId,
        notify: true
      })).then(() => {
        this.audioEl.play();
        this.videoEl.play();
        this.sendFileData(reliableChannel, unreliableChannel);
      });
    });
  }

  sendFileData(reliableChannel, unreliableChannel) {
    const dataFile = this.props.squawker.dataFile;
    if (!dataFile) { return; }

    const reader = new FileReader();
    reader.onload = () => {
      const messages = JSON.parse(reader.result);
      const start = performance.now();
      let index = 0;
      const userId = this.props.squawker.userId;
      const messageIntervalId = setInterval(() => {
        const time = performance.now() - start;
        let message = messages[index];
        while (time >= message.time) {
          if (message.message.data.owner) {
            message.message.data.owner = userId;
          }
          if (message.message.data.networkId) {
            message.message.data.networkId += userId;
          }
          if (message.message.data.parent) {
            message.message.data.parent += userId;
          }
          message.message.clientId = userId;

          try {
            const channel = message.reliable ? reliableChannel : unreliableChannel;
            channel.send(JSON.stringify(message.message));
          }
          catch(e) {
            console.error('Failed to send file data', e);
            clearInterval(messageIntervalId);
            break;
          }

          index++;
          message = messages[index];
          if (index === messages.length) {
            clearInterval(messageIntervalId);
            break;
          }
        }
      }, 10);
    }
    reader.readAsText(dataFile);
  }



  render() {
    const squawker = this.props.squawker;
    const handleId = this.state.handle != null ? this.state.handle.id : null;
    return (
      e("article", { className: "squawker" },
        e("h1", {},
          "User ID: ",
          e("span", { className: "user-id" }, squawker.userId.toString()),
          " Handle ID: ",
          e("span", { className: "handle-id" }, handleId)),
        e("audio", { crossOrigin: 'anonymous', controls: true, src: squawker.audioUrl, ref: (audio) => this.audioEl = audio }),
        e("video", { controls: true, src: squawker.videoUrl, ref: (video) => this.videoEl = video })));
  }
}

class SquawkerList extends React.Component {
  render() {
    const items = this.props.squawkers.map((x, i) => e(SquawkerItem, Object.assign({}, this.props, { squawker: x, key: x.userId })));
    return e("section", {}, items);
  }
}

class AddSquawkerForm extends React.Component {
  constructor(props) {
    super(props);
    this.create = this.create.bind(this);
  }

  generateUserId() {
    return Math.floor(Math.random() * (1000000001));
  }

  create(e) {
    this.props.onCreate(new Squawker(
      this.audioFile.files.length == 0 ? null : this.audioFile.files[0],
      this.videoFile.files.length == 0 ? null : this.videoFile.files[0],
      this.dataFile.files.length == 0 ? null : this.dataFile.files[0],
      this.generateUserId()
    ));
    e.preventDefault();
  }

  render() {
    return (
      e("form", { onSubmit: this.create },
        e("label", {}, "Audio file: ",
          e("input", { type: "file", ref: (input) => this.audioFile = input })),
        e("label", {}, "Video file: ",
          e("input", { type: "file", ref: (input) => this.videoFile = input })),
        e("label", {}, "Data file: ",
          e("input", { type: "file", ref: (input) => this.dataFile = input })),
        e("input", { type: "submit", value: "Create" })));
  }
}

class SquawkerApp extends React.Component {
  constructor(props) {
    super(props);
    this.state = { squawkers: [] };
    this.onCreate = this.onCreate.bind(this);
  }

  componentWillMount() {
    this.establishSession(this.props.ws, this.props.session);
  }

  establishSession(ws, session) {
    ws.addEventListener("open", () => {
      ws.addEventListener("message", this.handleMessage.bind(this));
      session.create().then(() => this.setState({ created: true }));
    });
  }

  handleMessage(ev) {
    var data = JSON.parse(ev.data);
    this.props.session.receive(data);
  }

  onCreate(squawker) {
    this.setState({ squawkers: this.state.squawkers.concat([squawker]) });
  }

  render() {
    if (this.state.created) {
      return (
        e("div", {id: "app"},
          e("p", {},
            "Connected to ",
            e("span", { className: "janus-url"}, this.props.ws.url),
            " with session ID: ",
            e("span", { className: "session-id" }, this.props.session.id)),
          e("h2", {}, "Add squawker"),
          e(AddSquawkerForm, {onCreate: this.onCreate}),
          e("h2", {}, "Existing squawkers"),
          e(SquawkerList, {roomId: this.props.roomId, session: this.props.session, squawkers: this.state.squawkers})));
    } else {
      return (
        e("div", {id: "app"},
          e("p", {}, "Connecting to Janus..."),
          e("div", { className: "loader" })));
    }
  }
}

const params = new URLSearchParams(location.search.slice(1));
const serverUrl = params.get("janus") || `wss://${location.hostname}:8989`;
const roomId = params.get("room") || 0;
const ws = new WebSocket(serverUrl, "janus-protocol");
const session = new Minijanus.JanusSession(ws.send.bind(ws));
const root = document.getElementById("root");
ReactDOM.render(e(SquawkerApp, { ws: ws, session: session, roomId: parseInt(roomId) }), root);
