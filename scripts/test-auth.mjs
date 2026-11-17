// Run with: node --experimental-vm-modules --test scripts/test-auth.mjs
// Exercise the real browser modules with a small DOM and controlled API responses.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createContext, SourceTextModule } from 'node:vm';

const appSource = readFileSync(new URL('../web/assets/app.js', import.meta.url), 'utf8');
const loginSource = readFileSync(new URL('../web/assets/pages/login.js', import.meta.url), 'utf8');

class Element {
  innerHTML = '';
  value = '';
  disabled = false;
  checked = true;
  dataset = {};
  classList = { add() {}, remove() {}, toggle() {} };
  children = new Map();
  querySelector(key) {
    if (!this.children.has(key)) this.children.set(key, new Element());
    return this.children.get(key);
  }
  querySelectorAll() { return []; }
  append() {}
  replaceWith() {}
  remove() {}
}

async function browser({ path = '/login', search = '', respond = () => ({ user: null }) } = {}) {
  const elements = new Element();
  const requests = [];
  const redirects = [];
  let reloads = 0;
  const document = {
    querySelector: (key) => elements.querySelector(key),
    querySelectorAll: () => [],
    getElementById: (id) => elements.querySelector(`#${id}`),
    createElement: () => new Element(),
    addEventListener() {},
  };
  const context = createContext({
    document, history: {}, URL, URLSearchParams, Intl, setTimeout, clearTimeout,
    sessionStorage: { setItem() {}, removeItem() {} },
    location: {
      origin: 'https://example.test', pathname: path, search,
      replace: (url) => redirects.push(url), reload: () => reloads++,
    },
    fetch: async (url, options) => {
      requests.push({ url, ...options });
      const result = url === '/api/meta' ? { mail: true } : await respond(url, options);
      return { ok: true, text: async () => JSON.stringify(result) };
    },
  });
  const app = new SourceTextModule(appSource, { context });
  await app.link(() => { throw new Error('app.js has no static imports'); });
  await app.evaluate();
  return {
    app: app.namespace, requests, redirects, elements,
    reloads: () => reloads,
    async login() {
      elements.querySelector('#go').disabled = true;
      elements.querySelector('#email').value = 'student@example.test';
      elements.querySelector('#password').value = 'test-password';
      elements.querySelector('#password2').value = 'test-password';
      const login = new SourceTextModule(loginSource, { context });
      await login.link(() => app);
      await login.evaluate();
      await flush();
    },
    submit: () => elements.querySelector('#f').onsubmit({ preventDefault() {} }),
  };
}

const flush = () => new Promise((resolve) => setImmediate(resolve));
const user = { id: 1, nickname: 'student', level: 1 };

test('a failed identity lookup rejects and the next lookup retries without browser caching', async () => {
  let attempts = 0;
  const b = await browser({ respond: () => {
    if (++attempts === 1) throw new Error('offline');
    return { user };
  } });
  await assert.rejects(b.app.me(), /网络连接失败/);
  assert.equal((await b.app.me()).id, user.id);
  assert.equal(attempts, 2);
  assert.ok(b.requests.every((r) => r.cache === 'no-store' && r.credentials === 'same-origin'));
});

test('an old failed lookup cannot discard a new session lookup', async () => {
  let failOld;
  let attempts = 0;
  const b = await browser({ respond: () => ++attempts === 1
    ? new Promise((_, reject) => { failOld = reject; }) : { user } });
  const old = b.app.me();
  const rejected = assert.rejects(old);
  b.app.forgetMe();
  const current = b.app.me();
  await current;
  failOld(new Error('old connection failed'));
  await rejected;
  assert.equal(b.app.me(), current);
  assert.equal(attempts, 2);
});

test('identity lookup failure on a protected page offers retry without redirecting', async () => {
  const b = await browser({ path: '/me', respond: () => { throw new Error('offline'); } });
  void b.app.layout('me');
  await flush();
  assert.equal(b.redirects.length, 0);
  assert.match(b.elements.querySelector('main').innerHTML, /暂时无法确认登录状态/);
  b.elements.querySelector('#retry-session').onclick();
  assert.equal(b.reloads(), 1);
});

test('a confirmed guest still goes to login with the original destination', async () => {
  const b = await browser({ path: '/browse', search: '?q=math' });
  void b.app.layout('browse');
  await flush();
  assert.equal(b.redirects[0], '/login?next=%2Fbrowse%3Fq%3Dmath');
});

for (const path of ['/login', '/login.html']) {
  test(`the login link on ${path} does not nest the login page inside next`, async () => {
    const b = await browser({ path, search: '?next=%2Fbrowse' });
    assert.equal(b.app.loginUrl(), '/login');
  });
}

for (const target of ['/login', '/login?next=%2Fbrowse', '/login.html#reset']) {
  test(`signing in never returns to another login page: ${target}`, async () => {
    let authenticated = false;
    const b = await browser({ search: `?next=${encodeURIComponent(target)}`, respond: (url) => {
      if (url.startsWith('/api/auth/')) authenticated = true;
      return { user: authenticated ? user : null };
    } });
    await b.login();
    await b.submit();
    assert.equal(b.redirects[0], '/me');
  });
}

for (const mode of ['login', 'register', 'reset']) {
  test(`${mode} confirms the new session before returning to the requested page`, async () => {
    let authenticated = false;
    const b = await browser({ search: `?mode=${mode}&next=%2Fbrowse`, respond: (url) => {
      if (url.startsWith('/api/auth/')) authenticated = true;
      return { user: authenticated ? user : null };
    } });
    await b.login();
    assert.equal(b.elements.querySelector('#go').disabled, false);
    await b.submit();
    assert.equal(b.redirects[0], '/browse');
    assert.deepEqual(b.requests.filter((r) => r.url !== '/api/meta').map((r) => r.url),
      ['/api/me', `/api/auth/${mode}`, '/api/me']);
  });
}

test('blocked cookies show a useful error and keep the login form available', async () => {
  const b = await browser();
  await b.login();
  await b.submit();
  assert.equal(b.redirects.length, 0);
  assert.match(b.elements.querySelector('#msg').innerHTML, /允许此网站使用 Cookie/);
  assert.equal(b.elements.querySelector('#go').disabled, false);
});

test('a failed initial identity lookup does not prevent signing in', async () => {
  let authenticated = false;
  const b = await browser({ respond: (url) => {
    if (url.startsWith('/api/auth/')) authenticated = true;
    if (!authenticated) throw new Error('offline');
    return { user };
  } });
  await b.login();
  assert.equal(b.elements.querySelector('#go').disabled, false);
  await b.submit();
  assert.equal(b.redirects[0], '/me');
});

test('a connection failure after signing in stays on the form and allows another attempt', async () => {
  let authenticated = false;
  let failOnce = true;
  const b = await browser({ respond: (url) => {
    if (url.startsWith('/api/auth/')) authenticated = true;
    else if (authenticated && failOnce) { failOnce = false; throw new Error('offline'); }
    return { user: authenticated ? user : null };
  } });
  await b.login();
  await b.submit();
  assert.equal(b.redirects.length, 0);
  assert.match(b.elements.querySelector('#msg').innerHTML, /网络连接失败/);
  assert.equal(b.elements.querySelector('#go').disabled, false);
  await b.submit();
  assert.equal(b.redirects[0], '/me');
});

test('the initial HTML disables native submission while modules are loading', () => {
  const html = readFileSync(new URL('../web/login.html', import.meta.url), 'utf8');
  const button = html.match(/<button\b[^>]*\bid="go"[^>]*>/)?.[0];
  assert.match(button, /\bdisabled\b/);
  assert.match(button, /\btype="submit"/);
});
