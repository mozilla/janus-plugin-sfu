var janus = null;
var publisher = null;
var id = "retproxy-" + Janus.randomString(12);
var user_id = null;

function getQueryParams(qs) {
    qs = qs.split('+').join(' ');

    var params = {},
        tokens,
        re = /[?&]?([^=]+)=([^&]*)/g;

    while ((tokens = re.exec(qs))) {
        params[decodeURIComponent(tokens[1])] = decodeURIComponent(tokens[2]);
    }

    return params;
}

var params = getQueryParams(document.location.search);
var sendAudio = params.send;

function init() {
    // Create session
    janus = new Janus({
        server: "ws://localhost:8188",
//	server: "wss://quander.me:8989",
	// No "iceServers" is provided, meaning janus.js will use a default STUN server
	// Here are some examples of how an iceServers field may look like to support TURN
	// 		iceServers: [{urls: "turn:yourturnserver.com:3478", username: "janususer", credential: "januspwd"}],
	// 		iceServers: [{urls: "turn:yourturnserver.com:443?transport=tcp", username: "janususer", credential: "januspwd"}],
	// 		iceServers: [{urls: "turns:yourturnserver.com:443?transport=tcp", username: "janususer", credential: "januspwd"}],
	// Should the Janus API require authentication, you can specify either the API secret or user token here too
	//		token: "mytoken",
	//	or
	//		apisecret: "serversecret",
	success: function() {
	    // Attach to echo test plugin
	    janus.attach({
		plugin: "janus.plugin.retproxy",
                opaqueId: id,
		success: function(pluginHandle) {
		    publisher = pluginHandle;
		    Janus.log("Plugin attached! (" + publisher.getPlugin() + ", id=" + publisher.getId() + ")");
		    // Negotiate WebRTC
		    Janus.debug("Trying a createOffer");
		    publisher.createOffer({
                        media: {audioSend: params.send !== undefined, audioRecv: false, video: false, data: true},
			success: function(jsep) {
			    Janus.debug("Got SDP!");
			    Janus.debug(jsep);
			    publisher.send({"message": {"kind": "join", "role": "publisher"}, "jsep": jsep});
			},
			error: function(error) {
			    Janus.error("WebRTC error:", error);
			}
		    });
		},
		error: function(error) {
		    console.error("  -- Error attaching plugin...", error);
		},
		consentDialog: function(on) {
		    Janus.debug("Consent dialog should be " + (on ? "on" : "off") + " now");
		},
		iceState: function(state) {
		    Janus.log("ICE state changed to " + state);
		},
		mediaState: function(medium, on) {
		    Janus.log("Janus " + (on ? "started" : "stopped") + " receiving our " + medium);
		},
		webrtcState: function(on) {
		    Janus.log("Janus says our WebRTC PeerConnection is " + (on ? "up" : "down") + " now");
		},
		slowLink: function(uplink, nacks) {
		    Janus.warn("Janus reports problems " + (uplink ? "sending" : "receiving") +
			       " packets on this PeerConnection (" + nacks + " NACKs/s " + (uplink ? "received" : "sent") + ")");
		},
		onmessage: function(msg, jsep) {
		    Janus.debug(" ::: Got a message :::");
		    Janus.debug(JSON.stringify(msg));
                    if(msg["event"] === "join_self") {
                        user_id = msg["user_id"];
                        var user_ids = msg["user_ids"];
                        for (var i = 0; i < user_ids.length; i++) {
                            var target_id = user_ids[i];
                            if (user_id !== target_id) {
                                Janus.debug("Creating subscriber to " + target_id);
                                createSubscriber(user_id, target_id);
                            }
                        }
                    }
                    if(msg["event"] === "join_other") {
                        var target_id = msg["user_id"];
                        if (user_id !== target_id) {
                            Janus.debug("Creating subscriber to " + target_id);
                            createSubscriber(user_id, target_id);
                        }
                    }
		    if(jsep !== undefined && jsep !== null) {
			Janus.debug("Handling SDP as well...");
			Janus.debug(jsep);
			publisher.handleRemoteJsep({jsep: jsep});
		    }
		},
		onlocalstream: function(stream) {
		    Janus.debug(" ::: Got a local stream in publisher :::");
		    Janus.debug(JSON.stringify(stream));
		},
		onremotestream: function(stream) {
		    Janus.debug(" ::: Got a remote stream in publisher :::");
		    Janus.debug(JSON.stringify(stream));
		},
		ondataopen: function(data) {
		    Janus.log("The DataChannel is available!");
		},
		ondata: function(data) {
		    Janus.debug("We got data from the DataChannel! " + data);
		},
		oncleanup: function() {
		    Janus.log(" ::: Got a cleanup notification :::");
		}
	    });

	},
	error: function(error) {
	    Janus.error(error);
	},
	destroyed: function() {
	    window.location.reload();
	}
    });
}

function createSubscriber(user_id, target_id) {
    var subscriber = null;
    janus.attach({
	plugin: "janus.plugin.retproxy",
        opaqueId: id,
	success: function(pluginHandle) {
	    subscriber = pluginHandle;
	    Janus.log("Plugin attached! (" + subscriber.getPlugin() + ", id=" + subscriber.getId() + ")");
	    // Negotiate WebRTC
	    Janus.debug("Trying a createOffer");
	    subscriber.createOffer({
                media: {audioRecv: true, audioSend: false, video: false, data: false},
		success: function(jsep) {
		    Janus.debug("Got SDP!");
		    Janus.debug(jsep);
		    subscriber.send({"message": {"kind": "join", "role": "subscriber", "user_id": user_id, "target_id": target_id}, "jsep": jsep});
		},
		error: function(error) {
		    Janus.error("WebRTC error:", error);
		}
	    });
	},
	error: function(error) {
	    console.error("  -- Error attaching plugin...", error);
	},
	consentDialog: function(on) {
	    Janus.debug("Consent dialog should be " + (on ? "on" : "off") + " now");
	},
	iceState: function(state) {
	    Janus.log("ICE state changed to " + state);
	},
	mediaState: function(medium, on) {
	    Janus.log("Janus " + (on ? "started" : "stopped") + " receiving our " + medium);
	},
	webrtcState: function(on) {
	    Janus.log("Janus says our WebRTC PeerConnection is " + (on ? "up" : "down") + " now");
	},
	slowLink: function(uplink, nacks) {
	    Janus.warn("Janus reports problems " + (uplink ? "sending" : "receiving") +
		       " packets on this PeerConnection (" + nacks + " NACKs/s " + (uplink ? "received" : "sent") + ")");
	},
	onmessage: function(msg, jsep) {
	    Janus.debug(" ::: Got a message :::");
	    Janus.debug(JSON.stringify(msg));
	    if(jsep !== undefined && jsep !== null) {
		Janus.debug("Handling SDP as well...");
		Janus.debug(jsep);
		subscriber.handleRemoteJsep({jsep: jsep});
	    }
	},
	onlocalstream: function(stream) {
	    Janus.debug(" ::: Got a local stream in subscriber :::");
	    Janus.debug(JSON.stringify(stream));
	},
	onremotestream: function(stream) {
	    Janus.debug(" ::: Got a remote stream in subscriber :::");
	    Janus.debug(JSON.stringify(stream));
            var el = document.createElement("audio");
            document.body.appendChild(el);
            Janus.attachMediaStream(el, stream);
            el.play();
	},
	ondataopen: function(data) {
	    Janus.log("The DataChannel is available!");
	},
	ondata: function(data) {
	    Janus.debug("We got data from the DataChannel! " + data);
	},
	oncleanup: function() {
	    Janus.log(" ::: Got a cleanup notification :::");
	}
    });

}
