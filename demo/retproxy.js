var janus = null;
var retproxy = null;
var id = "retproxy-" + Janus.randomString(12);

function init() {
    // Create session
    janus = new Janus({
	server: "http://10.252.26.59:8088/janus",
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
		    retproxy = pluginHandle;
		    Janus.log("Plugin attached! (" + retproxy.getPlugin() + ", id=" + retproxy.getId() + ")");
		    // Negotiate WebRTC
		    Janus.debug("Trying a createOffer");
		    retproxy.createOffer({
                        media: {audio: false, video: false, data: true},
			success: function(jsep) {
			    Janus.debug("Got SDP!");
			    Janus.debug(jsep);
			    retproxy.send({"message": {"kind": "publisher"}, "jsep": jsep});
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
                    if(msg["publishers"] !== undefined && msg["publishers"] !== null) {
                        var list = msg["publishers"];
			Janus.debug("Got a list of available publishers/feeds:");
			Janus.debug(list);
			for(var f in list) {
			    var id = list[f]["id"];
			    Janus.debug("  >> [" + id + "] ");
			    newRemoteFeed(id);
			}
                    }
		    if(jsep !== undefined && jsep !== null) {
			Janus.debug("Handling SDP as well...");
			Janus.debug(jsep);
			retproxy.handleRemoteJsep({jsep: jsep});
		    }
		},
		onlocalstream: function(stream) {
		    Janus.debug(" ::: Got a local stream :::");
		    Janus.debug(JSON.stringify(stream));
		},
		onremotestream: function(stream) {
		    Janus.debug(" ::: Got a remote stream :::");
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

function newRemoteFeed(id) {
    // A new feed has been published, create a new plugin handle and attach to it as a listener
    var remoteFeed = null;
    janus.attach({
	plugin: "janus.plugin.retproxy",
	opaqueId: id,
	success: function(pluginHandle) {
	    remoteFeed = pluginHandle;
	    Janus.log("Plugin attached! (" + remoteFeed.getPlugin() + ", id=" + remoteFeed.getId() + ")");
	    Janus.log("  -- This is a subscriber");
	    // We wait for the plugin to send us an offer
	    remoteFeed.send({"message": {"kind": "listener"}});
	},
	error: function(error) {
	    Janus.error("  -- Error attaching plugin...", error);
	},
	onmessage: function(msg, jsep) {
	    Janus.debug(" ::: Got a message (listener) :::");
	    Janus.debug(JSON.stringify(msg));
	    if(jsep !== undefined && jsep !== null) {
		Janus.debug("Handling SDP...");
		Janus.debug(jsep);
		// Answer and attach
		remoteFeed.createAnswer({
		    jsep: jsep,
		    media: {video: false, audioRecv: true, audioSend: false, data: false},
		    success: function(jsep) {
			Janus.debug("Got SDP!");
			Janus.debug(jsep);
			remoteFeed.send({"message": {"kind": "listener"}, "jsep": jsep});
		    },
		    error: function(error) {
			Janus.error("WebRTC error:", error);
		    }
		});
	    }
	},
	webrtcState: function(on) {
	    Janus.log("Janus says this WebRTC PeerConnection (feed #" + remoteFeed.rfindex + ") is " + (on ? "up" : "down") + " now");
	},
	onlocalstream: function(stream) {
	    // The subscriber stream is recvonly, we don't expect anything here
	},
	onremotestream: function(stream) {
	    Janus.log(" ::: Got a remote stream! :::");
            Janus.attachMediaStream(document.getElementById("audio"), stream);
	},
	oncleanup: function() {
	    Janus.log(" ::: Got a cleanup notification (remote feed " + id + ") :::");
	}
    });
}
