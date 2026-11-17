// Run with: node --experimental-vm-modules --test scripts/test-downloads.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createContext, SourceTextModule } from 'node:vm';

const source = readFileSync(new URL('../web/assets/app.js', import.meta.url), 'utf8');
const user = { id: 7, nickname: 'student', level: 1 };
const plan = { account: user.id, filename: 'notes.txt', size: 3, parts: [{ size: 3, urls: ['https://mirror.invalid/file'] }] };

async function browser({ identity = () => ({ user, downloaded_resources: [] }), record = () => ({}), mirrorFails = false, channelType } = {}) {
  const requests = [], events = [];
  const handlers = new Map();
  let clock = Date.now();
  class Clock extends Date { static now() { return clock; } }
  const badge = { dataset: { downloaded: '42' }, hidden: true };
  const location = { origin: 'https://example.test', pathname: '/browse', search: '', href: '', reload() { events.push('reload'); } };
  class BrowserURL extends URL {
    static createObjectURL() { return 'blob:test'; }
    static revokeObjectURL() {}
  }
  const document = {
    visibilityState: 'hidden',
    addEventListener(type, fn) { handlers.set(type, fn); },
    querySelector: () => null,
    querySelectorAll: () => [badge],
    createElement: (tag) => ({ tag, click() { events.push('save'); }, remove() {} }),
    body: { appendChild(el) { if (el.tag === 'div') events.push('toast'); } },
  };
  const context = createContext({
    document, location, history: {}, Date: Clock, URL: BrowserURL, URLSearchParams, Intl, Blob, AbortController, performance,
    sessionStorage: { removeItem() {} }, BroadcastChannel: channelType,
    setTimeout(fn, ms) { const timer = setTimeout(fn, ms); timer.unref(); return timer; }, clearTimeout,
    fetch: async (url, options) => {
      requests.push({ url, ...options });
      if (!url.startsWith('/api/')) {
        if (mirrorFails) throw new Error('mirror unavailable');
        let done = false;
        return { ok: true, body: { getReader: () => ({ read: async () => {
          if (done) return { done: true };
          done = true;
          return { done: false, value: new Uint8Array([1, 2, 3]) };
        } }) } };
      }
      let value;
      if (url === '/api/me') value = await identity();
      else if (url.endsWith('/downloaded')) { events.push('record'); value = await record(options); }
      else if (url.endsWith('/download')) value = plan;
      else throw new Error(`unexpected API ${url}`);
      return { ok: true, text: async () => JSON.stringify(value) };
    },
  });
  const mod = new SourceTextModule(source, { context });
  await mod.link(() => { throw new Error('unexpected static import'); });
  await mod.evaluate();
  return {
    app: mod.namespace, badge, requests, events, location, advance(ms) { clock += ms; },
    visible() { document.visibilityState = 'visible'; handlers.get('visibilitychange')(); },
  };
}

test('account history populates badges without browser-local storage', async () => {
  const b = await browser({ identity: () => ({ user, downloaded_resources: [42] }) });
  await b.app.me();
  assert.equal(b.badge.hidden, false);
  assert.doesNotMatch(b.app.downloadedBadge(42), / hidden/);
  assert.match(b.app.downloadedBadge(43), / hidden/);
});

test('a fresh browser sees the same account history', async () => {
  const identity = () => ({ user, downloaded_resources: [42] });
  const a = await browser({ identity });
  const b = await browser({ identity });
  await Promise.all([a.app.me(), b.app.me()]);
  assert.equal(a.app.downloadedBadge(42), b.app.downloadedBadge(42));
  assert.equal(b.badge.hidden, false);
});

test('sign-out and another account clear all previous download marks', async () => {
  let current = { user, downloaded_resources: [42] };
  const b = await browser({ identity: () => current });
  await b.app.me();
  b.app.forgetMe();
  assert.equal(b.badge.hidden, true);
  current = { user: { ...user, id: 8 }, downloaded_resources: [43] };
  await b.app.me();
  assert.match(b.app.downloadedBadge(42), / hidden/);
  assert.doesNotMatch(b.app.downloadedBadge(43), / hidden/);
});

test('an old identity response cannot paint another account history', async () => {
  let finishOld, calls = 0;
  const b = await browser({ identity: () => ++calls === 1
    ? new Promise((resolve) => { finishOld = resolve; })
    : { user: { ...user, id: 8 }, downloaded_resources: [43] } });
  const old = b.app.me();
  b.app.forgetMe();
  await b.app.me();
  finishOld({ user, downloaded_resources: [42] });
  await old;
  assert.match(b.app.downloadedBadge(42), / hidden/);
  assert.doesNotMatch(b.app.downloadedBadge(43), / hidden/);
});

test('a stale history refresh cannot erase a download recorded while it was in flight', async () => {
  let finishRefresh, calls = 0;
  const b = await browser({ identity: () => ++calls === 1
    ? { user, downloaded_resources: [] }
    : new Promise((resolve) => { finishRefresh = resolve; }) });
  await b.app.me();
  b.advance(61_000);
  const refresh = b.app.me();
  await b.app.rememberDownload(42, user.id);
  finishRefresh({ user, downloaded_resources: [] });
  await refresh;
  assert.equal(b.badge.hidden, false);
});

test('an old account metadata write cannot paint the newly signed-in account', async () => {
  let finishRecord;
  let current = { user, downloaded_resources: [] };
  const b = await browser({ identity: () => current, record: () => new Promise((resolve) => { finishRecord = resolve; }) });
  await b.app.me();
  const write = b.app.rememberDownload(42, user.id);
  b.app.forgetMe();
  current = { user: { ...user, id: 8 }, downloaded_resources: [43] };
  await b.app.me();
  finishRecord({});
  await write;
  assert.match(b.app.downloadedBadge(42), / hidden/);
  assert.doesNotMatch(b.app.downloadedBadge(43), / hidden/);
});

test('returning to a tab refreshes identity and removes the previous account marks', async () => {
  let current = { user, downloaded_resources: [42] };
  const b = await browser({ identity: () => current });
  await b.app.me();
  current = { user: { ...user, id: 8 }, downloaded_resources: [43] };
  b.visible();
  assert.equal(b.badge.hidden, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.match(b.app.downloadedBadge(42), / hidden/);
  assert.doesNotMatch(b.app.downloadedBadge(43), / hidden/);
  assert.ok(b.events.includes('reload'));
});

test('overlapping foreground refreshes coalesce without reloading an unchanged account', async () => {
  let finishRefresh, calls = 0;
  const b = await browser({ identity: () => ++calls === 1
    ? { user, downloaded_resources: [42] }
    : new Promise((resolve) => { finishRefresh = resolve; }) });
  await b.app.me();
  b.visible();
  b.visible();
  assert.equal(calls, 2);
  finishRefresh({ user, downloaded_resources: [42] });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(b.events.includes('reload'), false);
  assert.equal(b.badge.hidden, false);
});

test('a superseded foreground refresh cannot reload after explicit account switching', async () => {
  let finishRefresh, calls = 0;
  const b = await browser({ identity: () => {
    calls++;
    if (calls === 1) return { user, downloaded_resources: [42] };
    if (calls === 2) return new Promise((resolve) => { finishRefresh = resolve; });
    return { user: { ...user, id: 8 }, downloaded_resources: [43] };
  } });
  await b.app.me();
  b.visible();
  b.app.forgetMe();
  await b.app.me();
  finishRefresh({ user, downloaded_resources: [42] });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(b.events.includes('reload'), false);
  assert.match(b.app.downloadedBadge(42), / hidden/);
});

test('a download finishing during identity refresh is painted after the account is confirmed', async () => {
  let finishRefresh, calls = 0;
  const b = await browser({ identity: () => ++calls === 1
    ? { user, downloaded_resources: [] }
    : new Promise((resolve) => { finishRefresh = resolve; }) });
  await b.app.me();
  b.visible();
  const write = b.app.rememberDownload(42, user.id);
  await new Promise((resolve) => setImmediate(resolve));
  finishRefresh({ user, downloaded_resources: [] });
  await write;
  assert.equal(b.badge.hidden, false);
});

test('a failed visibility refresh does not leave the old account history displayed', async () => {
  let offline = false;
  const b = await browser({ identity: () => {
    if (offline) throw new Error('offline');
    return { user, downloaded_resources: [42] };
  } });
  await b.app.me();
  offline = true;
  b.visible();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(b.badge.hidden, true);
  assert.match(b.app.downloadedBadge(42), / hidden/);
});

test('authentication changes broadcast to other open tabs without storing account history', async () => {
  const channels = [];
  class Channel {
    constructor() { channels.push(this); }
    addEventListener(_type, fn) { this.receive = fn; }
    postMessage() { for (const c of channels) if (c !== this) c.receive(); }
  }
  let current = { user, downloaded_resources: [42] };
  const a = await browser({ identity: () => current, channelType: Channel });
  const b = await browser({ identity: () => current, channelType: Channel });
  await Promise.all([a.app.me(), b.app.me()]);
  current = { user: { ...user, id: 8 }, downloaded_resources: [43] };
  a.app.forgetMe();
  assert.equal(b.badge.hidden, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.match(b.app.downloadedBadge(42), / hidden/);
  assert.doesNotMatch(b.app.downloadedBadge(43), / hidden/);
});

test('a guest cannot display history from a stale identity payload', async () => {
  const b = await browser({ identity: () => ({ user: null, downloaded_resources: [42] }) });
  await b.app.me();
  assert.equal(b.badge.hidden, true);
});

test('file save precedes the account metadata write and updates visible marks', async () => {
  const b = await browser();
  await b.app.me();
  const result = await b.app.downloadResource(42);
  assert.equal(result.ok, true);
  assert.deepEqual(b.events, ['save', 'record']);
  assert.equal(b.badge.hidden, false);
  const req = b.requests.find((r) => r.url.endsWith('/downloaded'));
  assert.equal(req.method, 'POST');
  assert.equal(req.headers['X-XMUHub'], '1');
  assert.deepEqual(JSON.parse(req.body), { account: user.id });
});

test('a failed history write does not repeat or downgrade a successful file save', async () => {
  const b = await browser({ record: () => { throw new Error('offline'); } });
  await b.app.me();
  const result = await b.app.downloadResource(42);
  assert.equal(result.ok, true);
  assert.equal(b.events.filter((x) => x === 'save').length, 1);
  assert.equal(b.requests.filter((r) => !r.url.startsWith('/api/')).length, 1);
  assert.equal(b.badge.hidden, true);
  assert.equal(b.location.href, '');
});

test('single-file direct fallback records before leaving the page', async () => {
  const b = await browser({ mirrorFails: true });
  await b.app.me();
  const result = await b.app.downloadResource(42);
  assert.equal(result.fallback, true);
  assert.deepEqual(b.events, ['record']);
  assert.equal(b.location.href, plan.parts[0].urls[0]);
});

test('failed batch downloads are not marked downloaded', async () => {
  const b = await browser({ mirrorFails: true });
  await b.app.me();
  const result = await b.app.downloadResource(42, () => {}, { batch: true });
  assert.equal(result.ok, false);
  assert.equal(b.requests.some((r) => r.url.endsWith('/downloaded')), false);
  assert.equal(b.badge.hidden, true);
});
