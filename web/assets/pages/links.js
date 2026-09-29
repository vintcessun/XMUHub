import { ago, api, esc, layout, linkify, loginUrl, toast, $ } from '../app.js';

// 站外资源: outside collections. Everyone reads the list; signed-in users (staff too) suggest
// links, which another reviewer adds on the admin page (「站外链接」). Reviewers fix an entry's
// name, note and order, or remove it, right here; a new address is a new suggestion.

const host = (url) => { try { return new URL(url).host; } catch { return ''; } };

let staff = false;
let list = [];

function render() {
  $('#links').innerHTML = list.length
    ? list.map((l) => `<div class="item" data-id="${l.id}"><div class="body">
        <a class="title" href="${esc(l.url)}" target="_blank" rel="noopener noreferrer nofollow">${esc(l.title)}</a>
        ${l.note ? `<div class="note">${linkify(l.note)}</div>` : ''}
        <div class="meta"><span class="faint mono">${esc(host(l.url))}</span></div></div>
        ${staff ? '<div class="row" style="flex:none"><button class="btn sm" data-edit type="button">编辑</button><button class="btn sm danger" data-del type="button">移除</button></div>' : ''}
      </div>`).join('')
    : '<div class="empty"><b>还没有推荐</b></div>';
}

function form(l) {
  return `<section class="card"><h3>编辑链接</h3>
    <div class="fields-2">
      <label class="field"><span>名称 <em>*</em></span><input class="input" id="ltitle" maxlength="40" value="${esc(l?.title || '')}" placeholder="{{site.link_example}}"></label>
      <label class="field"><span>链接 <em>*</em></span><input class="input" id="lurl" maxlength="400" value="${esc(l.url)}" readonly title="换地址请重新推荐"></label>
    </div>
    <label class="field"><span>说明（选填：是什么、适合谁、提取码等）</span><input class="input" id="lnote" maxlength="200" value="${esc(l?.note || '')}"></label>
    <div class="row"><label class="small">排序 <input class="input" id="lsort" type="number" min="0" value="${l?.sort ?? 100}" style="max-width:90px"></label>
      <button class="btn primary sm" id="lsave" type="button">保存</button><button class="btn sm" id="lcancel" type="button">取消</button>
      <span class="small muted">数字小的排在前面。</span></div></section>`;
}

function showForm(l) {
  $('#manage').innerHTML = form(l);
  $('#lsave').onclick = async () => {
    const body = { title: $('#ltitle').value, url: $('#lurl').value, note: $('#lnote').value, sort: Number($('#lsort').value) || 0 };
    try {
      await api(`/links/${l.id}`, { method: 'PATCH', body });
      toast('已保存');
      await load();
      $('#manage').innerHTML = '';
    } catch (e) { toast(e.message, true); }
  };
  $('#lcancel').onclick = () => { $('#manage').innerHTML = ''; };
}

const SUGG = { pending: ['等待审核', 'pending'], approved: ['已采纳', 'published'], rejected: ['未采纳', 'rejected'] };

function suggestForm() {
  $('#sform').innerHTML = `<div class="fields-2">
      <label class="field"><span>名称 <em>*</em></span><input class="input" id="stitle" maxlength="40" placeholder="{{site.link_example}}"></label>
      <label class="field"><span>链接 <em>*</em></span><input class="input" id="surl" maxlength="400" placeholder="https://…"></label></div>
    <label class="field"><span>说明（选填：是什么、适合谁）</span><input class="input" id="snote" maxlength="200"></label>
    <div class="row"><button class="btn primary sm" id="ssend" type="button">提交推荐</button></div>`;
  $('#ssend').onclick = async () => {
    try {
      await api('/links/suggestions', { method: 'POST', body: { title: $('#stitle').value, url: $('#surl').value, note: $('#snote').value } });
      toast('已提交，审核员核实后会加到列表里');
      suggestForm();
      mine();
    } catch (e) { toast(e.message, true); }
  };
}

async function mine() {
  const list = await api('/links/suggestions').catch(() => []);
  const own = list.filter((s) => s.mine);
  $('#mine').innerHTML = own.length ? `<h4 style="margin:16px 0 6px">我推荐的</h4><div class="list">${own.map((s) => {
    const [label, cls] = SUGG[s.status] || [s.status, ''];
    return `<div class="item"><div class="body"><b>${esc(s.title)}</b> <span class="faint mono small">${esc(s.url)}</span>
      <div class="meta"><span class="badge ${cls}">${label}</span><span>${ago(s.created_at)}</span>${s.review_note ? `<span style="color:var(--bad)">原因：${esc(s.review_note)}</span>` : ''}</div></div></div>`;
  }).join('')}</div>` : '';
}

async function load() {
  try { list = await api('/links'); } catch (e) { $('#links').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  render();
}

(async () => {
  const me = await layout('links');
  staff = !!me && me.level >= 3;
  await load();
  if (me) { suggestForm(); mine(); } else $('#sform').innerHTML = `<p class="small"><a href="${loginUrl()}">登录</a>后可以推荐。</p>`;
  if (!staff) return;
  $('#links').onclick = async (e) => {
    const row = e.target.closest('[data-id]');
    if (!row) return;
    const l = list.find((x) => String(x.id) === row.dataset.id);
    if (e.target.closest('[data-edit]')) { showForm(l); $('#manage').scrollIntoView({ behavior: 'smooth' }); }
    if (e.target.closest('[data-del]') && confirm(`从列表里移除「${l.title}」？`)) {
      try { await api(`/links/${l.id}`, { method: 'DELETE' }); toast('已移除'); load(); } catch (err) { toast(err.message, true); }
    }
  };
})();
