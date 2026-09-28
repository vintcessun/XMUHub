import { api, esc, layout, linkify, toast, $ } from '../app.js';

// 站外资源: a list staff keep of outside collections. Everyone reads it; reviewers and admins
// add, edit and remove entries right here.

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

function form(l = null) {
  return `<section class="card"><h3>${l ? '编辑链接' : '添加链接'}</h3>
    <div class="fields-2">
      <label class="field"><span>名称 <em>*</em></span><input class="input" id="ltitle" maxlength="40" value="${esc(l?.title || '')}" placeholder="如 XMU-CS-exam（信息学院试卷）"></label>
      <label class="field"><span>链接 <em>*</em></span><input class="input" id="lurl" maxlength="400" value="${esc(l?.url || '')}" placeholder="https://…"></label>
    </div>
    <label class="field"><span>说明（选填：是什么、适合谁、提取码等）</span><input class="input" id="lnote" maxlength="200" value="${esc(l?.note || '')}"></label>
    <div class="row"><label class="small">排序 <input class="input" id="lsort" type="number" min="0" value="${l?.sort ?? 100}" style="max-width:90px"></label>
      <button class="btn primary sm" id="lsave" type="button">${l ? '保存' : '添加'}</button>${l ? '<button class="btn sm" id="lcancel" type="button">取消</button>' : ''}
      <span class="small muted">数字小的排在前面。</span></div></section>`;
}

function showForm(l = null) {
  $('#manage').innerHTML = form(l);
  $('#lsave').onclick = async () => {
    const body = { title: $('#ltitle').value, url: $('#lurl').value, note: $('#lnote').value, sort: Number($('#lsort').value) || 0 };
    try {
      await api(l ? `/links/${l.id}` : '/links', { method: l ? 'PATCH' : 'POST', body });
      toast(l ? '已保存' : '已添加');
      await load();
      showForm();
    } catch (e) { toast(e.message, true); }
  };
  if (l) $('#lcancel').onclick = () => showForm();
}

async function load() {
  try { list = await api('/links'); } catch (e) { $('#links').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  render();
}

(async () => {
  const me = await layout('links');
  staff = !!me && me.level >= 3;
  await load();
  if (!staff) return;
  showForm();
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
