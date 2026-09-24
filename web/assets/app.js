// 鹭岛书阁 shared front-end helpers: API client, session, layout, formatting, downloads.
// Plain ES modules, no build step — edit and redeploy.

export const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { v == null ? localStorage.removeItem(k) : localStorage.setItem(k, v); } catch { /* private mode */ } },
};

export class ApiError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}

/** JSON API call. Every non-GET carries the anti-CSRF header the server requires. */
export async function api(path, { method = 'GET', body, signal } = {}) {
  const headers = {};
  if (method !== 'GET') headers['X-XMUHub'] = '1';
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  let res;
  try {
    res = await fetch(`/api${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal, credentials: 'same-origin' });
  } catch (e) {
    if (e.name === 'AbortError') throw e;
    throw new ApiError(0, '网络连接失败，请稍后重试');
  }
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch { /* non-JSON */ }
  if (!res.ok) throw new ApiError(res.status, (data && data.error) || `请求失败（HTTP ${res.status}）`);
  return data;
}

let meCache = null;
/** The signed-in user or null. */
export function me() {
  if (!meCache) meCache = api('/me').then((r) => r.user).catch(() => null);
  return meCache;
}
export function forgetMe() {
  meCache = null;
  try { sessionStorage.removeItem('ludao.top'); } catch { /* storage blocked */ }
}

let metaCache = null;
export function meta() {
  if (!metaCache) metaCache = api('/meta');
  return metaCache;
}

let treeCache = null;
/** The whole category tree: { list, byId, children(id) }. */
export function tree() {
  if (!treeCache) {
    treeCache = api('/tree').then((list) => {
      const byId = new Map(list.map((n) => [n.id, n]));
      const kids = new Map();
      for (const n of list) {
        const k = n.parent ?? 0;
        if (!kids.has(k)) kids.set(k, []);
        kids.get(k).push(n);
      }
      return { list, byId, children: (id) => kids.get(id ?? 0) || [] };
    });
  }
  return treeCache;
}

/** Site slogan, shown big on the home page. */
export const SLOGAN = '让每一份资料都被需要它的人找到';
/** User group shown on the feedback page, e.g. 'QQ 群 123456789'; empty hides it. */
export const REPO_URL = 'https://github.com/vintcessun/XMUHub';

export const COMMUNITY = 'QQ 群：1106047582';

export const LEVELS = ['访客', '贡献者', '可信贡献者', '审核员', '管理员'];

// ---------------------------------------------------------------- formatting

export function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

export function fmtSize(n) {
  if (n == null) return '';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n >= 100 || i === 0 ? Math.round(n) : n.toFixed(1)} ${u[i]}`;
}

export function fmtDate(sec, withTime = false) {
  if (!sec) return '';
  const d = new Date(sec * 1000);
  const p = (x) => String(x).padStart(2, '0');
  const day = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  return withTime ? `${day} ${p(d.getHours())}:${p(d.getMinutes())}` : day;
}

export function ago(sec) {
  const s = Date.now() / 1000 - sec;
  if (s < 60) return '刚刚';
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)} 天前`;
  return fmtDate(sec);
}

export const STATUS = { pending: '待审核', published: '已发布', rejected: '未通过', removed: '已下架', restricted: '仅内部' };

export function statusBadge(r) {
  const bits = [];
  if (r.status !== 'published') bits.push(`<span class="badge ${esc(r.status)}">${STATUS[r.status] || esc(r.status)}</span>`);
  else if (r.needs_review) bits.push('<span class="badge pending">待复核</span>');
  if (r.uncertain) bits.push('<span class="badge uncertain">待核实</span>');
  return bits.join('');
}

export function pathText(path) {
  return (path || []).filter((p) => p.kind !== 'section').map((p) => p.name).join(' / ');
}

/** Display name of a node: levels ("上", "I-1") read badly alone, so use their label. */
export function nodeTitle(n) {
  return n.kind === 'level' && n.label && n.label !== n.name ? n.label : n.name;
}

export function stars(avg) {
  const full = Math.round(avg);
  return `<span class="stars" aria-label="${avg} 星">${'★'.repeat(full)}<i>${'★'.repeat(5 - full)}</i></span>`;
}

export function resourceItem(r, { showNode = true } = {}) {
  const e = r.ext || 'file';
  const bits = [
    `<span class="tag t-${esc(r.tag.code)}">${esc(r.tag.label)}</span>`,
    showNode ? `<a href="/n/${r.node.id}">${esc(nodeTitle(r.node))}</a>` : '',
    `<span>${fmtSize(r.size)}</span>`,
    `<span>${r.downloads} 次下载</span>`,
    r.rating && r.rating.count ? `<span title="${r.rating.count} 人评分">${stars(r.rating.avg)} ${r.rating.avg}</span>` : '',
    statusBadge(r),
  ].filter(Boolean);
  // First-page thumbnail when one has been made (see server thumbs.rs); else the type badge.
  const icon = r.thumb && r.thumb.length
    ? `<img class="thumb" src="${esc(r.thumb[0])}" data-alts="${esc(r.thumb.slice(1).join(' '))}" data-ext="${esc(e.slice(0, 4))}" loading="lazy" alt="" title="点击预览">`
    : `<div class="ficon ${esc(e)}">${esc(e.slice(0, 4))}</div>`;
  return `<div class="item">
    ${icon}
    <div class="body"><a class="title" href="/r/${r.id}">${esc(r.title)}</a>
      ${r.subtitle ? `<div class="subtitle">${esc(r.subtitle)}</div>` : ''}
      ${r.note ? `<div class="note">${esc(r.note)}</div>` : ''}
      <div class="meta">${bits.join('')}</div></div>
    <button class="btn sm qpv" data-qpv="${r.id}" type="button" title="不用点进去，直接看内容">预览</button>
  </div>`;
}

export function nodeCard(n, path) {
  const sub = path ? pathText(path) : (n.code || '');
  return `<a class="card node-card" href="/n/${n.id}">
    <h3>${esc(nodeTitle(n))}</h3>
    <div class="meta">${sub ? `<span>${esc(sub)}</span>` : ''}<span><b class="count">${n.count}</b> 份</span></div>
  </a>`;
}

export function toast(msg, bad = false) {
  const el = document.createElement('div');
  el.className = 'toast' + (bad ? ' bad' : '');
  el.textContent = msg;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), bad ? 4500 : 2500);
}

export const qs = new URLSearchParams(location.search);

/** Numeric id from a clean URL such as /n/12 or /r/34. */
export function pathId() {
  const m = /\/(\d+)\/?$/.exec(location.pathname);
  return m ? Number(m[1]) : null;
}

export function $(sel, root = document) { return root.querySelector(sel); }

export function loginUrl() {
  return `/login?next=${encodeURIComponent(location.pathname + location.search)}`;
}

// ---------------------------------------------------------------- layout

export async function layout(active) {
  const top = document.getElementById('top');
  const q = qs.get('q') || '';
  top.className = 'top';
  top.innerHTML = `<div class="wrap">
    <a class="brand" href="/"><img class="logo" src="/assets/logo.svg" alt=""><span><b>鹭岛书阁</b><small>厦大资料库</small></span></a>
    ${active === 'home' || active === 'search' ? '<span class="grow"></span>' : `<form action="/search" role="search"><input name="q" value="${esc(q)}" placeholder="搜索课程、资料…" aria-label="搜索"></form>`}
    <nav class="nav">
      <a href="/browse" data-k="browse">分类</a>
      <a href="/help" data-k="help">教程</a>
      <a href="https://github.com/vintcessun/XMUHub" class="gh" rel="noopener" target="_blank" title="本站完全开源，欢迎 Star 和参与开发">⭐ 开源</a>
      <a href="/admin" data-k="admin" hidden>审核</a>
      <a href="/me" data-k="me" id="nav-me">登录</a>
      <a href="/upload" data-k="upload" class="up">上传</a>
    </nav></div>`;
  top.querySelectorAll('.nav a').forEach((a) => { if (a.dataset.k === active && a.dataset.k !== 'upload') a.classList.add('on'); });
  const foot = document.getElementById('foot');
  foot.className = 'foot';
  foot.innerHTML = `<div class="wrap">
    <span>鹭岛书阁 · 厦门大学学生资料共享 · 非官方学生项目，与厦门大学官方无关</span>
    <span><a href="/help">使用教程</a> · <a href="/about">使用须知</a> · <a href="/feedback">意见反馈</a>${COMMUNITY ? `（${esc(COMMUNITY)}）` : ''} · <a href="https://github.com/vintcessun/XMUHub" rel="noopener">源代码（AGPL-3.0）</a> · 资料由同学上传，仅供学习交流</span></div>`;
  if (active !== 'feedback') feedbackButton();
  const user = await me();
  const navMe = top.querySelector('#nav-me');
  if (user) {
    navMe.textContent = user.nickname;
    if (user.level >= 3) top.querySelector('[data-k="admin"]').hidden = false;
  } else {
    navMe.href = loginUrl();
  }
  // The next page paints this header/footer before its scripts run, so switching pages
  // doesn't flash an empty bar (see the inline script after <header id="top">).
  try {
    sessionStorage.setItem('ludao.top', top.innerHTML);
    sessionStorage.setItem('ludao.foot', foot.innerHTML);
  } catch { /* storage blocked */ }
  speculate();
  return user;
}

/** Lets the browser load a page while the pointer rests on its link (Chrome/Edge), so the
 * click opens it at once. Browse-type pages are prerendered; resource pages (which may
 * start a preview download), upload and account pages are only prefetched. */
function speculate() {
  if (document.querySelector('script[type="speculationrules"]') || !HTMLScriptElement.supports?.('speculationrules')) return;
  const s = document.createElement('script');
  s.type = 'speculationrules';
  s.textContent = JSON.stringify({
    prerender: [{ where: { or: [{ href_matches: '/' }, { href_matches: '/browse' }, { href_matches: '/help' }, { href_matches: '/about' }, { href_matches: '/n/*' }, { href_matches: '/search?*' }] }, eagerness: 'moderate' }],
    prefetch: [{ where: { or: [{ href_matches: '/r/*' }, { href_matches: '/me' }, { href_matches: '/feedback' }, { href_matches: '/upload' }, { href_matches: '/upload?*' }] }, eagerness: 'moderate' }],
  });
  document.head.append(s);
}

// ---------------------------------------------------------------- downloads

function saveBlob(blob, name) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/**
 * Fetches one part, trying each URL in turn. A mirror that is reachable but crawling
 * (under ~150 KB/s after a few seconds) is abandoned for the next one.
 */
export async function fetchPart(part, urls, onBytes) {
  let lastErr;
  for (const [i, url] of urls.entries()) {
    const hasNext = i < urls.length - 1;
    const ctl = new AbortController();
    try {
      const startTimer = setTimeout(() => ctl.abort(), 15_000);
      const res = await fetch(url, { signal: ctl.signal, mode: 'cors', credentials: 'omit', referrerPolicy: 'no-referrer' });
      clearTimeout(startTimer);
      if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
      const reader = res.body.getReader();
      const chunks = [];
      const t0 = performance.now();
      let got = 0;
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        got += value.length;
        onBytes(got);
        const secs = (performance.now() - t0) / 1000;
        if (hasNext && secs > 6 && got / secs < 150 * 1024) { ctl.abort(); throw new Error('镜像太慢'); }
      }
      if (got !== part.size) throw new Error('大小不符');
      return new Blob(chunks);
    } catch (e) {
      lastErr = e;
      onBytes(0);
    }
  }
  throw lastErr || new Error('没有可用的下载地址');
}

/**
 * Downloads a resource. Single files: try a cross-origin fetch through the fastest mirror
 * (keeps the Chinese filename); most mirrors send no CORS headers, in which case we navigate
 * to that same fastest mirror (full speed, ASCII filename). Multi-part files must be fetched
 * and joined in the browser, so every mirror is tried for each part.
 */
export async function downloadResource(id, onProgress = () => {}) {
  const plan = await api(`/resources/${id}/download`);
  const total = plan.size;
  const single = plan.parts.length === 1;
  let doneBytes = 0;
  try {
    const blobs = [];
    for (const part of plan.parts) {
      const urls = single ? part.urls.slice(0, 1) : part.urls;
      const b = await fetchPart(part, urls, (n) => onProgress(Math.min(1, (doneBytes + n) / total)));
      doneBytes += part.size;
      blobs.push(b);
    }
    saveBlob(new Blob(blobs, { type: plan.mime || 'application/octet-stream' }), plan.filename);
    return { ok: true };
  } catch (e) {
    if (single) {
      location.href = plan.parts[0].urls[0];
      return { ok: true, fallback: true };
    }
    return { ok: false, plan, error: e };
  }
}

// ---------------------------------------------------------------- modal, password toggle

/** Opens a modal dialog and returns its body element (with `.close()`). */
export function modal(title, { wide = false } = {}) {
  const wrap = document.createElement('div');
  wrap.className = 'modal';
  wrap.innerHTML = `<div class="modal-box${wide ? ' wide' : ''}" role="dialog" aria-modal="true">
    <div class="modal-head"><b>${esc(title)}</b><span class="grow"></span><button class="btn sm" data-close type="button">关闭</button></div>
    <div class="modal-body"></div></div>`;
  const onKey = (e) => { if (e.key === 'Escape') close(); };
  function close() { wrap.remove(); document.removeEventListener('keydown', onKey); }
  wrap.onclick = (e) => { if (e.target === wrap || e.target.closest('[data-close]')) close(); };
  document.addEventListener('keydown', onKey);
  document.body.appendChild(wrap);
  const body = wrap.querySelector('.modal-body');
  body.close = close;
  return body;
}

/** Adds a 显示/隐藏 button to each password input. */
export function pwToggle(...inputs) {
  for (const input of inputs) {
    if (!input || input.dataset.pw) continue;
    input.dataset.pw = '1';
    const box = document.createElement('span');
    box.className = 'pwbox';
    input.replaceWith(box);
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'pweye';
    b.textContent = '显示';
    b.onclick = () => {
      const show = input.type === 'password';
      input.type = show ? 'text' : 'password';
      b.textContent = show ? '隐藏' : '显示';
    };
    box.append(input, b);
  }
}

// ---------------------------------------------------------------- preview

/** Renders a preview of resource `id` into `box` (code loaded on first use). */
export async function preview(id, box) {
  const m = await import('./preview.js');
  return m.preview(id, box);
}

// ---------------------------------------------------------------- node picker

/** Asks for a target category (search by name / code / pinyin). Resolves to the node or null. */
export function pickNode(title = '选择分类') {
  return new Promise((resolve) => {
    const body = modal(title);
    body.innerHTML = `<div class="field suggest"><input class="input" placeholder="课程名 / 代号 / 拼音首字母，或直接输入分类 ID" autocomplete="off"></div>
      <ul class="picklist"></ul>`;
    const input = body.querySelector('input');
    const ul = body.querySelector('.picklist');
    let list = [];
    let seq = 0;
    let done = false;
    const finish = (n) => { if (done) return; done = true; body.close(); resolve(n); };
    const observer = new MutationObserver(() => { if (!body.isConnected) { observer.disconnect(); if (!done) { done = true; resolve(null); } } });
    observer.observe(document.body, { childList: true });
    input.oninput = async () => {
      const q = input.value.trim();
      const my = ++seq;
      if (!q) { ul.innerHTML = ''; return; }
      if (/^\d+$/.test(q)) {
        try { const d = await api(`/nodes/${q}`); if (my === seq) { list = [{ node: d.node, path: d.path }]; render(); } } catch { if (my === seq) ul.innerHTML = '<li class="faint">没有这个 ID</li>'; }
        return;
      }
      const r = await api(`/nodes/suggest?q=${encodeURIComponent(q)}`).catch(() => []);
      if (my !== seq) return;
      list = r;
      render();
    };
    function render() {
      ul.innerHTML = list.length
        ? list.map((x, i) => `<li data-i="${i}"><b>${esc(nodeTitle(x.node))}</b> <small class="faint">${esc(pathText(x.path))} · ID ${x.node.id}</small></li>`).join('')
        : '<li class="faint">没有找到</li>';
    }
    ul.onclick = (e) => { const li = e.target.closest('li[data-i]'); if (li) finish(list[Number(li.dataset.i)].node); };
    input.focus();
  });
}

/** Moves resources after asking for the target; resolves to the number moved (0 if cancelled). */
export async function moveResources(ids) {
  if (!ids.length) { toast('先勾选资料', true); return 0; }
  const n = await pickNode(`把 ${ids.length} 份资料移动到…`);
  if (!n) return 0;
  try {
    const r = await api('/resources/move', { method: 'POST', body: { ids, node: n.id } });
    toast(`已移动 ${r.moved} 份到「${n.name}」`);
    return r.moved;
  } catch (e) { toast(e.message, true); return 0; }
}

// ---------------------------------------------------------------- feedback button

/** A floating 「反馈」 button on every page; the form goes to the admin 反馈 tab. */
function feedbackButton() {
  if (document.querySelector('.fbfab')) return;
  const b = document.createElement('button');
  b.className = 'fbfab';
  b.type = 'button';
  b.textContent = '反馈';
  b.title = '意见反馈';
  b.onclick = () => {
    const body = modal('意见反馈');
    const DK = 'xmuhub.feedback.draft';
    let d = {};
    try { d = JSON.parse(store.get(DK) || '{}') || {}; } catch { /* ignore */ }
    body.innerHTML = `<form id="fbq">
      <p class="small muted" style="margin-top:0">遇到问题、想要新功能、资料分类不对，都可以说。${COMMUNITY ? `也可以加${esc(COMMUNITY)}。` : ''}</p>
      <label class="field"><span>反馈内容 <em>*</em></span><textarea class="input" id="fbq_b" rows="5" maxlength="2000" required placeholder="尽量写清楚：在哪个页面、做了什么、看到了什么。"></textarea></label>
      <label class="field"><span>联系方式（选填：邮箱 / QQ）</span><input class="input" id="fbq_c" maxlength="100"></label>
      <p class="small faint">会自动附上当前页面地址：${esc(location.pathname + location.search)}</p>
      <button class="btn primary">提交</button></form>`;
    const tb = body.querySelector('#fbq_b');
    const tc = body.querySelector('#fbq_c');
    tb.value = d.body || '';
    tc.value = d.contact || '';
    const save = () => store.set(DK, tb.value.trim() || tc.value.trim() ? JSON.stringify({ body: tb.value, contact: tc.value }) : null);
    tb.oninput = save;
    tc.oninput = save;
    tb.focus();
    body.querySelector('#fbq').onsubmit = async (e) => {
      e.preventDefault();
      const btn = body.querySelector('#fbq button');
      btn.disabled = true;
      try {
        await api('/feedback', { method: 'POST', body: { body: tb.value, contact: tc.value, page: location.pathname + location.search } });
        store.set(DK, null);
        body.innerHTML = '<div class="notice ok">谢谢！我们已经收到你的反馈。</div>';
        setTimeout(() => body.close(), 1600);
      } catch (err) { toast(err.message, true); btn.disabled = false; }
    };
  };
  document.body.appendChild(b);
}

// ---------------------------------------------------------------- quick preview from lists

/**
 * 「预览」 on any resource list opens the file in a dialog straight away; ← / → (or the
 * buttons) step through the other files of the same list, so comparing a handful of
 * candidates takes a click each instead of a page visit each.
 */
function quickPreview(btn) {
  const scope = btn.closest('.list') || document;
  const all = [...scope.querySelectorAll('[data-qpv]')];
  let i = all.indexOf(btn);
  const body = modal('', { wide: true });
  const head = body.parentElement.querySelector('.modal-head');
  head.querySelector('b').insertAdjacentHTML('afterend', '<span class="qpv-nav"><button class="btn sm" data-nav="-1" type="button" aria-label="上一份">‹ 上一份</button><span class="small faint" data-pos></span><button class="btn sm" data-nav="1" type="button" aria-label="下一份">下一份 ›</button><a class="btn sm primary" data-open>详情 / 下载</a></span>');
  const show = () => {
    const b = all[i];
    const item = b.closest('.item');
    const title = item?.querySelector('.title')?.textContent || '';
    const sub = item?.querySelector('.subtitle')?.textContent || '';
    head.querySelector('b').textContent = title;
    head.querySelector('[data-pos]').textContent = all.length > 1 ? `${i + 1} / ${all.length}` : '';
    head.querySelectorAll('[data-nav]').forEach((n) => { n.hidden = all.length < 2; });
    head.querySelector('[data-open]').href = `/r/${b.dataset.qpv}`;
    body.innerHTML = `${sub ? `<p class="small muted" style="margin:0 0 8px">${esc(sub)}</p>` : ''}<div class="qpv-review"></div><div></div>`;
    const id = Number(b.dataset.qpv);
    preview(id, body.lastElementChild);
    reviewBar(id, body.querySelector('.qpv-review'));
  };
  // Reviewers decide right in the dialog; the next file opens by itself.
  const reviewBar = async (id, bar) => {
    const u = await me();
    if (!u || u.level < 3) return;
    let r;
    try { r = await api(`/resources/${id}`); } catch { return; }
    if (!bar.isConnected) return;
    const acts = [
      (r.status !== 'published' || r.needs_review || r.uncertain) && ['approve', r.status === 'published' ? '确认无误' : '通过', 'ok'],
      (r.status === 'pending' || r.status === 'restricted' || r.needs_review || r.uncertain) && ['reject', '驳回', 'danger'],
      r.status === 'published' && ['remove', '下架', 'danger'],
      (r.status === 'removed' || r.status === 'rejected' || r.status === 'restricted') && ['restore', '恢复发布', ''],
    ].filter(Boolean);
    if (!acts.length) return;
    bar.innerHTML = `<div class="row" style="margin:0 0 10px;gap:6px"><span class="small muted">审核：${esc(STATUS[r.status] || r.status)}${r.needs_review ? ' · 待复核' : ''}${r.uncertain ? ' · 待核实' : ''}</span>
      <input class="input" data-note placeholder="备注（驳回 / 下架原因）" style="max-width:220px;min-height:30px;padding:3px 8px">
      ${acts.map(([a, l, c]) => `<button class="btn sm ${c}" data-act="${a}" type="button">${l}</button>`).join('')}</div>`;
    bar.onclick = async (e) => {
      const btn = e.target.closest('[data-act]');
      if (!btn) return;
      btn.disabled = true;
      try {
        const res = await api(`/resources/${id}/review`, { method: 'POST', body: { action: btn.dataset.act, note: bar.querySelector('[data-note]').value } });
        toast(`已处理：${STATUS[res.status] || res.status}`);
        // Take it out of the list behind the dialog and move on.
        const item = all[i].closest('[data-id]') || all[i].closest('.item');
        all.splice(i, 1);
        item?.remove();
        if (!all.length) { body.close(); return; }
        if (i >= all.length) i = 0;
        show();
      } catch (err) { toast(err.message, true); btn.disabled = false; }
    };
  };
  const go = (d) => { i = (i + d + all.length) % all.length; show(); };
  head.onclick = (e) => { const n = e.target.closest('[data-nav]'); if (n) go(Number(n.dataset.nav)); };
  const onKey = (e) => {
    if (!body.isConnected) { document.removeEventListener('keydown', onKey); return; }
    if (e.target.closest('input, textarea')) return;
    if (e.key === 'ArrowLeft') go(-1);
    if (e.key === 'ArrowRight') go(1);
  };
  document.addEventListener('keydown', onKey);
  show();
}

// A thumbnail that fails on one mirror tries the next, then falls back to the type badge.
document.addEventListener('error', (e) => {
  const img = e.target;
  if (!(img instanceof HTMLImageElement) || !img.classList.contains('thumb')) return;
  const alts = (img.dataset.alts || '').split(' ').filter(Boolean);
  if (alts.length) {
    img.dataset.alts = alts.slice(1).join(' ');
    img.src = alts[0];
  } else {
    const d = document.createElement('div');
    d.className = `ficon ${img.dataset.ext || ''}`;
    d.textContent = img.dataset.ext || 'file';
    img.replaceWith(d);
  }
}, true);

document.addEventListener('click', (e) => {
  const thumb = e.target.closest('img.thumb');
  const b = thumb ? thumb.closest('.item')?.querySelector('[data-qpv]') : e.target.closest('[data-qpv]');
  if (!b) return;
  e.preventDefault();
  quickPreview(b);
});
