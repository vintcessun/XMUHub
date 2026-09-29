import { ago, api, avatar, esc, layout, loginUrl, qs, resourceItem, SITE_NAME, toast, $ } from '../app.js';

// 收藏夹: without ?id, the reader's own lists (create, open) and the lists others shared;
// with ?id, one list. Its owner renames it, takes files off, asks to share it (a reviewer
// other than the owner approves), stops sharing or deletes it.

const STATUS = { private: ['仅自己可见', ''], pending: ['分享审核中', 'pending'], public: ['已公开', 'published'], rejected: ['分享未通过', 'rejected'] };
let me = null;

const badge = (s) => { const [l, c] = STATUS[s] || [s, '']; return `<span class="badge ${c}">${l}</span>`; };

function card(c) {
  return `<a class="card node-card" href="/collections?id=${c.id}"><h3>${esc(c.title)}</h3>
    <div class="small muted">${c.note ? esc(c.note) : '&nbsp;'}</div>
    <div class="small faint" style="margin-top:6px">${c.count} 份 · ${c.mine ? badge(c.status) : `${avatar(c.owner.avatar, c.owner.nickname, 18)} ${esc(c.owner.nickname)}`} · ${ago(c.updated_at)}</div></a>`;
}

async function home() {
  const box = $('#view');
  const [mine, shared] = await Promise.all([me ? api('/collections').catch(() => []) : Promise.resolve(null), api('/collections?scope=public').catch(() => [])]);
  box.innerHTML = `<div class="page-head"><div><h1>收藏夹</h1><div class="muted small">把常用的资料收进收藏夹，按课程、考试整理。收藏夹默认只有自己能看到；想分享给同学，点「申请公开分享」，审核员通过后大家就能看到。</div></div></div>
    ${me ? `<section class="card"><h3>我的收藏夹</h3>
      <div class="row" style="margin-bottom:12px"><input class="input" id="ctitle" maxlength="40" placeholder="新收藏夹的名字，如「高数期末」" style="max-width:260px"><button class="btn primary sm" id="cnew" type="button">新建</button></div>
      <div class="grid">${mine.map(card).join('') || '<p class="small faint">还没有收藏夹。在资料页点「收藏」也会自动建一个。</p>'}</div></section>`
      : `<section class="card"><p class="small"><a href="${loginUrl()}">登录</a>后可以收藏资料、建自己的收藏夹。</p></section>`}
    <section class="card"><h3>大家分享的</h3><div class="grid">${shared.map(card).join('') || '<p class="small faint">还没有公开分享的收藏夹。</p>'}</div></section>`;
  const add = box.querySelector('#cnew');
  if (add) {
    add.onclick = async () => {
      try {
        const c = await api('/collections', { method: 'POST', body: { title: box.querySelector('#ctitle').value } });
        location.href = `/collections?id=${c.id}`;
      } catch (e) { toast(e.message, true); }
    };
  }
}

async function one(id) {
  const box = $('#view');
  let d;
  try { d = await api(`/collections/${id}`); } catch (e) { box.innerHTML = `<div class="notice bad">${esc(e.message)}</div><p><a href="/collections">‹ 所有收藏夹</a></p>`; return; }
  const c = d.collection;
  document.title = `${c.title} · 收藏夹 · ${SITE_NAME}`;
  const shareBtn = c.status === 'private' || c.status === 'rejected'
    ? '<button class="btn sm" data-share="1" type="button">申请公开分享</button>'
    : '<button class="btn sm" data-share="0" type="button">取消分享</button>';
  box.innerHTML = `<div class="page-head"><div><div class="crumbs"><a class="uplevel" href="/collections">‹ 所有收藏夹</a></div>
      <h1>${esc(c.title)}</h1><div class="muted small">${c.note ? `${esc(c.note)} · ` : ''}${c.count} 份 · ${c.mine ? badge(c.status) : `${avatar(c.owner.avatar, c.owner.nickname, 18)} ${esc(c.owner.nickname)} 分享`}</div>
      ${c.mine && c.review_note ? `<div class="small" style="color:var(--bad);margin-top:4px">未通过原因：${esc(c.review_note)}</div>` : ''}</div>
    ${c.mine ? `<div class="row">${shareBtn}<button class="btn sm" id="cedit" type="button">改名</button><button class="btn sm danger" id="cdel" type="button">删除收藏夹</button></div>` : ''}</div>
    <div id="cform"></div>
    <section class="card"><div class="list">${d.items.map((x) => x.resource
      ? `<div data-r="${x.id}" class="${c.mine ? 'row' : ''}" style="${c.mine ? 'align-items:center;gap:6px' : ''}"><div style="flex:1;min-width:0">${resourceItem(x.resource)}</div>${c.mine ? '<button class="btn sm" data-off type="button">拿出</button>' : ''}</div>`
      : `<div class="item" data-r="${x.id}"><div class="body"><span class="faint">这份资料已下架或暂不可见（#${x.id}）</span></div><button class="btn sm" data-off type="button">拿出</button></div>`).join('')
      || `<div class="empty"><b>这个收藏夹还是空的</b>${c.mine ? '在资料页点「收藏」放进来' : ''}</div>`}</div></section>`;
  if (!c.mine) return;
  box.onclick = async (e) => {
    const off = e.target.closest('[data-off]');
    if (off) {
      try { await api(`/collections/${id}/items/${off.closest('[data-r]').dataset.r}`, { method: 'PUT', body: { on: false } }); off.closest('[data-r]').remove(); toast('已拿出'); } catch (err) { toast(err.message, true); }
      return;
    }
    const sh = e.target.closest('[data-share]');
    if (sh) {
      const on = sh.dataset.share === '1';
      try { await api(`/collections/${id}/share`, { method: 'POST', body: { on } }); toast(on ? '已提交，审核员通过后公开' : '已取消分享'); one(id); } catch (err) { toast(err.message, true); }
      return;
    }
    if (e.target.closest('#cdel')) {
      if (!confirm(`删除收藏夹「${c.title}」？里面的资料本身不受影响。`)) return;
      try { await api(`/collections/${id}`, { method: 'DELETE' }); location.href = '/collections'; } catch (err) { toast(err.message, true); }
      return;
    }
    if (e.target.closest('#cedit')) {
      $('#cform').innerHTML = `<section class="card"><label class="field"><span>名字</span><input class="input" id="et" maxlength="40" value="${esc(c.title)}"></label>
        <label class="field"><span>说明（选填）</span><input class="input" id="en" maxlength="200" value="${esc(c.note)}"></label>
        <div class="row"><button class="btn primary sm" id="esave" type="button">保存</button>${c.status === 'public' ? '<span class="small muted">已公开的收藏夹改名后要重新审核，审核期间别人看不到。</span>' : ''}</div></section>`;
      $('#esave').onclick = async () => {
        try { await api(`/collections/${id}`, { method: 'PATCH', body: { title: $('#et').value, note: $('#en').value } }); toast('已保存'); one(id); } catch (err) { toast(err.message, true); }
      };
    }
  };
}

(async () => {
  me = await layout('collections');
  const id = Number(qs.get('id'));
  if (id) one(id); else home();
})();
