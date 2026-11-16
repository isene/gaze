// A Web Audio output with nothing playing goes to sleep, and wakes for a
// sound. Run: node tests/quiet-audio.js   (prints "ok", or fails loudly)
const fs = require('fs'), assert = require('assert');
const src = fs.readFileSync(__dirname + '/../src/js.rs', 'utf8');
const from = src.indexOf('  (function () {\n    const AC = window.AudioContext');
const block = src.slice(from, src.indexOf('  })();\n', from) + 7);
assert(from > 0, 'the block is in js.rs');

let timers = [];
const setTimeout = (f) => { timers.push(f); return timers.length; };
const clearTimeout = (id) => { if (id) timers[id - 1] = null; };
const tick = () => { const t = timers; timers = []; t.forEach((f) => f && f()); };

class Target {
  constructor() { this.on = {}; }
  addEventListener(k, f) { (this.on[k] = this.on[k] || []).push(f); }
  fire(k) { (this.on[k] || []).forEach((f) => f()); }
}
class BaseAudioContext extends Target {
  constructor() { super(); this.state = 'running'; }
  suspend() { this.state = 'suspended'; this.fire('statechange'); return Promise.resolve(); }
  resume() { this.state = 'running'; this.fire('statechange'); return Promise.resolve(); }
  createMediaElementSource() { return {}; }
}
class AudioContext extends BaseAudioContext {}
class AudioBufferSourceNode extends Target {
  constructor(c) { super(); this.context = c; }
  start() { this.started = true; }
}
const window = { AudioContext, BaseAudioContext, AudioBufferSourceNode };
eval(block);

const c = new window.AudioContext();
assert(c instanceof AudioContext);
tick();
assert.equal(c.state, 'suspended', 'a silent output sleeps');
const s = new AudioBufferSourceNode(c);
s.start();
assert(s.started);
assert.equal(c.state, 'running', 'a sound wakes it');
tick();
assert.equal(c.state, 'running', 'it stays awake while the sound runs');
s.fire('ended');
tick();
assert.equal(c.state, 'suspended', 'and sleeps when the sound has ended');

const call = new window.AudioContext();
call.createMediaElementSource();
tick();
assert.equal(call.state, 'running', 'an output fed by a media element is left alone');

const own = new window.AudioContext();
tick();
own.resume();
tick();
assert.equal(own.state, 'suspended', 'a page that wakes a silent output gets no more than three seconds');
console.log('ok');
