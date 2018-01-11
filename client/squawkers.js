Minijanus.verbose = true;

const e = React.createElement;
const peerConfig = { iceServers: [{ urls: "stun:stun.l.google.com:19302" }] };
const params = new URLSearchParams(location.search.slice(1));

class Squawker {
  constructor(userId, conn, handle, data) {
    this.userId = userId;
    this.conn = conn;
    this.handle = handle;

    this.audioUrl = data.audioUrl || (data.audioFile && URL.createObjectURL(data.audioFile)) || null;
    this.videoUrl = data.videoUrl || (data.videoFile && URL.createObjectURL(data.videoFile)) || null;
    this.dataFile = data.dataFile;
    this.dataUrl = data.dataUrl;
  }

  negotiateIce() {
    return new Promise((resolve, reject) => {
      this.conn.addEventListener("icecandidate", ev => {
        this.handle.sendTrickle(ev.candidate || null).then(() => {
          if (!ev.candidate) { // this was the last candidate on our end and now they received it
            resolve();
          }
        });
      });
    });
  };
}

class SquawkerItem extends React.Component {

  componentDidMount() {
    var audioLoaded = this.props.squawker.audioUrl ? false : true;
    var videoLoaded = this.props.squawker.videoUrl ? false : true;
    var attachIfReady = () => {
      if (audioLoaded && videoLoaded) {
        this.attachPublisher(this.props.squawker);
      }
    };
    if (!audioLoaded) {
      this.audioEl.addEventListener("loadedmetadata", () => { audioLoaded = true; attachIfReady(); });
    }
    if (!videoLoaded) {
      this.videoEl.addEventListener("loadedmetadata", () => { videoLoaded = true; attachIfReady(); });
    }
    // workaround for broken `loop` attribute in headless chrome
    this.audioEl.addEventListener("timeupdate", () => {
      if (this.audioEl.currentTime > this.audioEl.duration - 1) { this.audioEl.currentTime = 0; }
    });
    this.videoEl.addEventListener("timeupdate", () => {
      if (this.videoEl.currentTime > this.videoEl.duration - 1) { this.videoEl.currentTime = 0; }
    });
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
    if (!this.props.squawker.audioUrl) { return null; }
    return this.captureStream(this.audioEl);
  }

  getVideoStream() {
    if (!this.props.squawker.videoUrl) { return null; }
    return this.captureStream(this.videoEl);
  }

  attachPublisher(squawker) {
    console.info("Attaching publisher for squawker: ", this.props.squawker.userId);
    var handle = squawker.handle;
    var conn = squawker.conn;
    return handle.attach("janus.plugin.sfu").then(() => {
      this.setState({ handle: handle });
      var iceReady = squawker.negotiateIce();

      const audioStream = this.getAudioStream();
      if (audioStream) {
        audioStream.getTracks().forEach(track => conn.addTrack(track, audioStream));
      }

      const videoStream = this.getVideoStream();
      if (videoStream) {
        videoStream.getTracks().forEach(track => conn.addTrack(track, videoStream));
      }

      const reliableChannel = conn.createDataChannel("reliable", { ordered: true });
      const unreliableChannel = conn.createDataChannel("unreliable", { ordered: false, maxRetransmits: 0 });

      var offerReady = conn.createOffer({ offerToReceiveAudio: 0, offerToReceiveVideo: 0 });
      var localReady = offerReady.then(offer => { console.log(offer); return offer; }).then(conn.setLocalDescription.bind(conn));
      var remoteReady = offerReady
          .then(handle.sendJsep.bind(handle))
          .then(answer => conn.setRemoteDescription(answer.jsep));
      var connectionReady = Promise.all([localReady, remoteReady]);
      return connectionReady.then(() => handle.sendMessage({
        kind: "join",
        room_id: this.props.roomId,
        user_id: this.props.squawker.userId,
        subscribe: { notify: true, data: true } // data = true necessary atm to send join notification
      })).then(() => {
        this.audioEl.play();
        this.videoEl.play();
        if (this.props.squawker.dataUrl || this.props.squawker.dataFile) {
          this.sendFileData(reliableChannel, unreliableChannel);
        }
      });
    });
  }

  async readAsText(file) {
    return new Promise(function (resolve, reject) {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.readAsText(file);
    });
  }

  async channelOpen(dataChannel) {
    return new Promise(function (resolve, reject) {
      if (dataChannel.readyState === "open") { resolve(); }
      else { dataChannel.onopen = resolve; }
    });
  }

  async getDataJson() {
    const dataUrl = this.props.squawker.dataUrl;
    if (dataUrl) {
      const response = await fetch(dataUrl);
      return response.text();
    }
    else {
      return await this.readAsText(this.props.squawker.dataFile);
    }
  }

  async sendFileData(reliableChannel, unreliableChannel) {
    const dataJson = await this.getDataJson();
    if (!dataJson) { return; }

    const messages = JSON.parse(dataJson);

    const userId = this.props.squawker.userId;
    messages.forEach(message => {
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
    });

    await this.channelOpen(reliableChannel);
    await this.channelOpen(unreliableChannel);

    let start = performance.now();
    let index = 0;
    const messageIntervalId = setInterval(() => {
      const time = performance.now() - start;
      let message = messages[index];
      while (time >= message.time) {
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
        if (index === messages.length) {
          if (params.get("automate")) {
            index = 0;
            start = performance.now();
          }
          else {
            clearInterval(messageIntervalId);
          }
          break;
        }
        message = messages[index];
      }
    }, 10);
  }

  render() {
    const squawker = this.props.squawker;
    return (
      e("article", { className: "squawker" },
        e("h1", {},
          "User ID: ",
          e("span", { className: "user-id" }, squawker.userId.toString())),
        e("audio", { crossOrigin: 'anonymous', controls: true, src: squawker.audioUrl, ref: (audio) => this.audioEl = audio }),
        e("video", { crossOrigin: 'anonymous', controls: true, src: squawker.videoUrl, ref: (video) => this.videoEl = video })));
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
    var data = {
      audioUrl: params.get("audioUrl"),
      videoUrl: params.get("videoUrl"),
      dataUrl: params.get("dataUrl"),
      audioFile: this.audioFile.files.length == 0 ? null : this.audioFile.files[0],
      videoFile: this.videoFile.files.length == 0 ? null : this.videoFile.files[0],
      dataFile: this.dataFile.files.length == 0 ? null : this.dataFile.files[0]
    };
    this.props.onCreate(new Squawker(
      this.generateUserId(),
      new RTCPeerConnection(peerConfig),
      new Minijanus.JanusPluginHandle(this.props.session),
      data
    ));
    if (e) { e.preventDefault(); }
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

  haveForm(form) {
    if (this.form) { return; }
    this.form = form;
    const num = parseInt(params.get("automate"), 10);
    const delay = parseInt(params.get("delay"), 10);
    if (!num) { return; }
    for (let i = 0; i < num; i++) {
      setTimeout(() => {
        form.create();
      }, delay * 1000 * i);
    }
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
          e(AddSquawkerForm, {onCreate: this.onCreate, session: this.props.session, ref: this.haveForm.bind(this)}),
          e("h2", {}, "Existing squawkers"),
          e(SquawkerList, {roomId: this.props.roomId, squawkers: this.state.squawkers})));
    } else {
      return (
        e("div", {id: "app"},
          e("p", {}, "Connecting to Janus..."),
          e("div", { className: "loader" })));
    }
  }
}

const serverUrl = params.get("janus") || `wss://${location.hostname}:8989`;
const roomId = params.get("room") || 0;
const ws = new WebSocket(serverUrl, "janus-protocol");
const session = new Minijanus.JanusSession(ws.send.bind(ws));
const root = document.getElementById("root");
ReactDOM.render(e(SquawkerApp, { ws: ws, session: session, roomId: parseInt(roomId) }), root);
