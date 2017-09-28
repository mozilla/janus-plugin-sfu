var janus = null;
var retproxy = null;

function init() {
    // Create session
    janus = new Janus({
	server: "http://localhost:8088/janus",
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
	    janus.attach(
		{
		    plugin: "janus.plugin.retproxy",
		    success: function(pluginHandle) {
			retproxy = pluginHandle;
			Janus.log("Plugin attached! (" + retproxy.getPlugin() + ", id=" + retproxy.getId() + ")");
			// Negotiate WebRTC
			Janus.debug("Trying a createOffer");
			retproxy.createOffer(
			    {
				success: function(jsep) {
				    Janus.debug("Got SDP!");
				    Janus.debug(jsep);
				    retproxy.send({"message": {}, "jsep": jsep});
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
                        Janus.attachMediaStream(document.getElementById("audio"), stream);

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
