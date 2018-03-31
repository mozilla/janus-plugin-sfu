#!/usr/bin/env node
const doc = `
Usage:
    ./squawkers-cli.js [options]

Options:
    -r --room=<room>        Room id [default: 2011].
    -n --num=<num>          Number of squawkers [default: 5].
    -l --delay=<secs>       Seconds of delay between creating each squawker [default: 5].
    -R --refresh=<secs>     If specified, continuously reload the client after this many seconds to simulate clients 
                            leaving the room.
    -j --janus=<url>        Janus server url [default: wss://dev-janus.reticulum.io].
    -a --audio=<url>        Url for audio file [default: https://ucarecdn.com/3b8bbf6a-c18e-4662-a4a2-99749802cbdc/].
    -d --data=<url>         Url for data file [default: https://ucarecdn.com/1c202eb0-2903-4065-9b67-298350eb2400/].
    -v --video=<url>        Url for video file.
    -h --help               Show this screen.
`;

// video: https://ucarecdn.com/ee5981c8-019d-4f6a-9dcd-118d71d1dcda/

const docopt = require('docopt').docopt;
const options = docopt(doc);

const puppeteer = require('puppeteer');
const querystring = require('querystring');

(async () => {
    const browser = await puppeteer.launch();
    const page = await browser.newPage();
    const params = {
        janus: options['--janus'],
        room: options['--room'],
        audioUrl: options['--audio'],
        videoUrl: options['--video'],
        dataUrl: options['--data'],
        automate: options['--num'],
        delay: options['--delay'],
    };
    console.log(params);

    const url = 'file:///' + __dirname + '/squawkers.html?' + querystring.stringify(params);

    console.log('spawning squawkers...');
    await page.goto(url);

    if (options['--refresh']) {
        setInterval(() => {
            console.log('reloading...');
            page.reload();
        }, parseInt(options['--refresh'], 10) * 1000);
    }
})();
