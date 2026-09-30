import { ago, api, esc, fmtDate, layout, linkify, loginUrl, pathText, setBell, toast, $ } from '../app.js';

// 站内提醒: the admins' 公告 on top, then new files where the reader follows and decisions on
// what they submitted. Opening one marks it read. Below: the courses they follow.

const KIND = { bulletin: '公告', move: '分类建议', feedback: '反馈', course: '关注', approved: '审核', upload: '审核', want: '求资料', link: '站外资源', question: '提问', avatar: '头像', change: '申请', collection: '收藏夹' };

async function load() {
  let d;
  try { d = await api('/notices?limit=200'); } catch (e) { $('#nlist').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  setBell(d.unread);
  $('#readall').disabled = !d.unread;
  $('#nlist').innerHTML = d.items.length
    ? d.items.map((n) => `<a class="notice-item ${n.read ? 'read' : 'unread'}" data-id="${n.id}" data-read="${n.read ? 1 : ''}" href="${esc(n.link || '#')}">
        <div class="body" style="flex:1"><div class="ntext">${esc(n.text)}</div>
        <div class="small faint"><span class="tag">${KIND[n.kind] || '提醒'}</span> ${ago(n.created_at)}</div></div></a>`).join('')
    : '<div class="empty"><b>还没有提醒</b>关注几门课，有新资料时这里会告诉你</div>';
}

async function bulletins() {
  let list;
  try { list = await api('/bulletins'); } catch { return; }
  $('#bulletins').hidden = !list.length;
  $('#blist').innerHTML = list.map((b) => `<div class="bulletin" id="b${b.id}"><div style="white-space:pre-wrap;overflow-wrap:anywhere">${linkify(b.text)}</div>
    <div class="small faint" style="margin-top:4px">${fmtDate(b.at, true)}</div></div>`).join('');
  // A reminder links to /notices#b<id>: the list only exists now, so scroll to it by hand.
  const target = location.hash && document.getElementById(location.hash.slice(1));
  if (target) { target.classList.add('on'); target.scrollIntoView({ block: 'center' }); }
}

async function follows() {
  let list;
  try { list = await api('/me/follows'); } catch (e) { $('#flist').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  $('#flist').innerHTML = list.length
    ? `<div class="list">${list.map((x) => `<div class="item" data-n="${x.node.id}"><div class="body"><a class="title" href="/n/${x.node.id}">${esc(x.node.name)}</a>
        <div class="meta"><span class="faint">${esc(pathText(x.path))}</span></div></div><button class="btn sm" data-unf type="button">取消关注</button></div>`).join('')}</div>`
    : '<p class="small faint">还没有关注任何课程。</p>';
}

$('#nlist').addEventListener('click', (e) => {
  const a = e.target.closest('[data-id]');
  if (!a || a.dataset.read) return;
  // Mark it read on the way out (keepalive: still sent as the page changes).
  fetch('/api/notices/read', { method: 'POST', keepalive: true, headers: { 'Content-Type': 'application/json', 'X-XMUHub': '1' }, body: JSON.stringify({ id: Number(a.dataset.id) }) }).catch(() => {});
});
$('#readall').onclick = async () => {
  try { await api('/notices/read', { method: 'POST', body: {} }); toast('已全部标为已读'); load(); } catch (e) { toast(e.message, true); }
};
$('#flist').addEventListener('click', async (e) => {
  const b = e.target.closest('[data-unf]');
  if (!b) return;
  const id = b.closest('[data-n]').dataset.n;
  try { await api(`/nodes/${id}/follow`, { method: 'PUT', body: { on: false } }); toast('已取消关注'); follows(); } catch (err) { toast(err.message, true); }
});

(async () => {
  const me = await layout('notices');
  if (!me) { location.replace(loginUrl()); return; }
  bulletins();
  load();
  follows();
})();
