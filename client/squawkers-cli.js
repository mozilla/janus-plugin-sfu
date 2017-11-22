#!/usr/bin/env node
const doc = `
Usage:
    ./squawkers-cli.js [options]

Options:
    -r --room=<room>        Room id [default: 2011].
    -n --num=<num>          Number of squawkers [default: 5].
    -l --delay=<delay>      Seconds of delay between creating each squawker [default: 5].
    -j --janus=<janus>      Janus server url [default: wss://dev-janus.reticulum.io].
    -a --audio=<audio>      Url for audio file [default: https://ucarecdn.com/c690e31e-70e2-4500-bbdd-4b83bfe3e156/].
    -d --data=<data>        Url for data file [default: https://ucarecdn.com/b0696343-bca0-41a1-ad9e-7d5c491b258f/].
    -v --video=<video>      Url for video file.
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
    console.log(__dirname);

    console.log('spawning squawkers...');
    await page.goto(url);
})();
