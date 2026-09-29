import { ago, api, avatar, esc, layout, linkify, loginUrl, pickFromTree, qs, store, toast, $ } from '../app.js';

// 求资料: students post what they are looking for (public once a reviewer approves it); others
// say 「我也要」, reply with a hint or a link to the file on the site, and the author marks it found.

const STATUS = { pending: ['待审核', 'pending'], open: ['求助中', ''], found: ['已找到', 'published'], closed: ['已关闭', ''], rejected: ['未通过', 'rejected'] };
const DRAFT = 'xmuhub.want.draft';

let me = null;
let tab = 'open';
let node = Number(qs.get('node')) || null;
let nodeName = '';
let openId = Number(qs.get('id')) || null;

/** 「/r/123」, a full resource address, or a bare id → the id. */
function resourceId(s) {
  const m = /(?:\/r\/)?(\d+)\s*$/.exec(String(s || '').trim());
  return m ? Number(m[1]) : null;
}

function who(p) {
  return `${avatar(p.avatar, p.nickname, 22)}<b>${esc(p.nickname)}</b>${p.role ? `<span class="badge published">${esc(p.role)}</span>` : ''}`;
}

function itemHtml(w) {
  const [label, cls] = STATUS[w.status] || [w.status, ''];
  return `<div class="item want" data-id="${w.id}"><div class="body">
      <a class="title" href="/wants?id=${w.id}" data-open>${esc(w.title)}</a>
      ${w.body ? `<div class="note">${linkify(w.body)}</div>` : ''}
      ${w.resource ? `<div class="small">找到了：<a href="/r/${w.resource.id}">${esc(w.resource.name)}</a></div>` : ''}
      ${w.review_note ? `<div class="small" style="color:var(--bad)">未通过：${esc(w.review_note)}</div>` : ''}
      <div class="meta">${w.status !== 'open' ? `<span class="badge ${cls}">${label}</span>` : ''}
        ${w.node ? `<a href="/n/${w.node.id}">${esc(w.node.name)}</a>` : ''}
        <span class="who">${who(w.author)}</span><span>${ago(w.created_at)}</span>
        <a href="/wants?id=${w.id}" data-open>${w.replies ? `${w.replies} 条回复` : '回复'}</a></div>
      <div class="detail" hidden></div></div>
    ${w.status === 'open' ? `<button class="btn sm${w.voted ? ' primary' : ''}" data-vote type="button" title="我也在找这份资料">我也要 ${w.votes || ''}</button>` : w.votes ? `<span class="small faint" style="flex:none">${w.votes} 人也要</span>` : ''}
  </div>`;
}

async function load() {
  const box = $('#wants');
  box.innerHTML = '<div class="skeleton"></div>';
  let list;
  try {
    list = await api(`/wants?status=${tab}${node ? `&node=${node}` : ''}`);
  } catch (e) { box.innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  box.innerHTML = list.length ? list.map(itemHtml).join('')
    : `<div class="empty"><b>${tab === 'found' ? '还没有找到的' : tab === 'mine' ? '你还没有发过求助' : '现在没有求助'}</b>${tab === 'open' ? '找不到资料时，发一条试试' : ''}</div>`;
  if (openId) {
    const row = box.querySelector(`[data-id="${openId}"]`);
    if (row) { detail(row); row.scrollIntoView({ block: 'center' }); } else openAlone(openId);
    openId = null;
  }
}

/** A post opened by link that isn't in the current list (another tab, or the author's pending one). */
async function openAlone(id) {
  try {
    const d = await api(`/wants/${id}`);
    $('#wants').insertAdjacentHTML('afterbegin', itemHtml(d.want));
    detail($('#wants').firstElementChild, d);
  } catch (e) { toast(e.message, true); }
}

async function detail(row, data = null) {
  const box = row.querySelector('.detail');
  if (!box.hidden && !data) { box.hidden = true; return; }
  box.hidden = false;
  box.innerHTML = '<div class="small faint">加载中…</div>';
  let d = data;
  try { d ||= await api(`/wants/${row.dataset.id}`); } catch (e) { box.innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  const w = d.want;
  const live = w.status === 'open' || w.status === 'found' || w.status === 'closed';
  box.innerHTML = `<div class="replies">${d.replies.map((r) => `<div class="comment" data-rid="${r.id}"><div class="who">${who(r.author)}<span class="faint">${ago(r.created_at)}</span><span class="grow"></span>${r.can_delete ? '<a href="#" data-rdel class="small">删除</a>' : ''}</div>
        ${r.body ? `<div class="text">${linkify(r.body)}</div>` : ''}${r.resource ? `<div class="small">资料：<a href="/r/${r.resource.id}">${esc(r.resource.name)}</a></div>` : ''}</div>`).join('') || (live ? '<p class="small faint">还没有回复</p>' : '')}</div>
    ${live && me ? `<form class="rform"><textarea class="input" name="body" rows="2" maxlength="500" placeholder="知道在哪？说一声：在哪门课下、群文件、老师的课程网站……"></textarea>
      <div class="row" style="margin-top:6px"><input class="input" name="res" placeholder="本站资料链接（选填，如 https://{{site.domain}}/r/123）" style="max-width:360px"><button class="btn sm primary">回复</button>
      <a class="small" href="/upload${w.node ? `?node=${w.node.id}` : ''}">我有，去上传</a></div></form>` : live ? `<p class="small"><a href="${loginUrl()}">登录</a>后可以回复</p>` : ''}
    ${w.can_manage && live ? `<div class="row small" style="margin-top:8px">${w.status === 'open'
      ? '<input class="input" name="found" placeholder="找到的资料链接（选填）" style="max-width:300px"><button class="btn sm" data-st="found" type="button">标记已找到</button><button class="btn sm" data-st="closed" type="button">关闭</button>'
      : '<button class="btn sm" data-st="open" type="button">重新打开</button>'}</div>` : ''}`;
  const form = box.querySelector('.rform');
  if (form) {
    form.onsubmit = async (e) => {
      e.preventDefault();
      const body = form.body.value.trim();
      const res = form.res.value.trim();
      if (res && !resourceId(res)) return toast('资料链接不对，应该像 /r/123', true);
      try {
        await api(`/wants/${w.id}/replies`, { method: 'POST', body: { body, resource: res ? resourceId(res) : null } });
        detail(row, await api(`/wants/${w.id}`));
      } catch (err) { toast(err.message, true); }
    };
  }
  box.onclick = async (e) => {
    const del = e.target.closest('[data-rdel]');
    if (del) {
      e.preventDefault();
      try { await api(`/want-replies/${del.closest('[data-rid]').dataset.rid}`, { method: 'DELETE' }); detail(row, await api(`/wants/${w.id}`)); } catch (err) { toast(err.message, true); }
      return;
    }
    const st = e.target.closest('[data-st]');
    if (!st) return;
    const found = box.querySelector('[name="found"]')?.value.trim();
    if (found && !resourceId(found)) return toast('资料链接不对，应该像 /r/123', true);
    try {
      await api(`/wants/${w.id}/status`, { method: 'POST', body: { status: st.dataset.st, resource: found ? resourceId(found) : null } });
      toast('已更新');
      load();
    } catch (err) { toast(err.message, true); }
  };
}

function compose() {
  const box = $('#compose');
  if (!me) { location.href = loginUrl(); return; }
  if (box.innerHTML) { box.innerHTML = ''; return; }
  let d = {};
  try { d = JSON.parse(store.get(DRAFT) || '{}') || {}; } catch { /* ignore */ }
  // Opened from a course page: the post is about that course unless changed.
  let pick = d.node || (node && nodeName ? { id: node, name: nodeName } : null);
  box.innerHTML = `<section class="card"><h3>发一条求助</h3>
    <label class="field"><span>想要什么 <em>*</em></span><input class="input" id="wt" maxlength="60" placeholder="如：数据结构 2023 期末试卷（带答案）"></label>
    <label class="field"><span>补充说明（选填）</span><textarea class="input" id="wb" rows="3" maxlength="500" placeholder="哪位老师的课、哪一年、要什么形式……"></textarea></label>
    <div class="field"><span>哪门课（选填）</span><div class="row"><span id="wn" class="small">${pick ? esc(pick.name) : '<span class="faint">没选</span>'}</span><button class="btn sm" id="wpick" type="button">选课程</button></div></div>
    <div class="row"><button class="btn primary sm" id="wsend" type="button">提交审核</button><span class="small muted">审核员通过后公开，一般一天内。</span></div></section>`;
  $('#wt').value = d.title || '';
  $('#wb').value = d.body || '';
  const save = () => store.set(DRAFT, $('#wt').value.trim() || $('#wb').value.trim() ? JSON.stringify({ title: $('#wt').value, body: $('#wb').value, node: pick }) : null);
  $('#wt').oninput = save;
  $('#wb').oninput = save;
  $('#wpick').onclick = async () => {
    const x = await pickFromTree('求的是哪门课的资料');
    if (!x) return;
    pick = { id: x.node.id, name: x.node.name };
    $('#wn').textContent = pick.name;
    save();
  };
  $('#wsend').onclick = async () => {
    try {
      const w = await api('/wants', { method: 'POST', body: { title: $('#wt').value, body: $('#wb').value, node: pick ? pick.id : null } });
      store.set(DRAFT, null);
      box.innerHTML = '';
      toast(w.status === 'open' ? '已发布' : '已提交，审核通过后公开');
      setTab('mine');
    } catch (e) { toast(e.message, true); }
  };
  $('#wt').focus();
}

function setTab(s) {
  tab = s;
  document.querySelectorAll('#tabs .chip').forEach((b) => b.classList.toggle('on', b.dataset.s === s));
  load();
}

$('#tabs').onclick = (e) => { const b = e.target.closest('[data-s]'); if (b) setTab(b.dataset.s); };
$('#newbtn').onclick = compose;
$('#wants').addEventListener('click', async (e) => {
  const row = e.target.closest('.want');
  if (!row) return;
  if (e.target.closest('[data-open]')) { e.preventDefault(); detail(row); return; }
  const v = e.target.closest('[data-vote]');
  if (!v) return;
  if (!me) { location.href = loginUrl(); return; }
  const on = !v.classList.contains('primary');
  try {
    const r = await api(`/wants/${row.dataset.id}/vote`, { method: 'PUT', body: { on } });
    v.classList.toggle('primary', on);
    v.textContent = `我也要 ${r.votes || ''}`;
  } catch (err) { toast(err.message, true); }
});

(async () => {
  me = await layout('wants');
  $('#tabs [data-s="mine"]').hidden = !me;
  if (node) {
    try {
      const d = await api(`/nodes/${node}`);
      nodeName = d.node.name;
      $('#nodefilter').innerHTML = `<p class="small">只看「<a href="/n/${node}">${esc(d.node.name)}</a>」的求助 · <a href="/wants" id="nfclear">看全部</a></p>`;
    } catch { node = null; }
  }
  load();
})();
