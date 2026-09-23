import { api, courseCard, layout, meta, resourceItem, $, esc } from '../app.js';

layout('home');

meta().then((m) => {
  const s = m.stats;
  $('#stats').innerHTML = `<span><b>${s.courses}</b>门课程</span><span><b>${s.resources}</b>份资料</span><span><b>${s.downloads}</b>次下载</span>`;
}).catch(() => {});

api('/courses').then((cs) => {
  $('#courses').innerHTML = cs.length
    ? cs.slice(0, 8).map(courseCard).join('')
    : `<div class="empty card" style="grid-column:1/-1"><b>还没有课程</b>成为第一个分享资料的人吧 → <a href="/upload">上传</a></div>`;
}).catch((e) => { $('#courses').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; });

for (const [id, path] of [['#recent', '/recent'], ['#popular', '/popular']]) {
  api(path).then((rs) => {
    $(id).innerHTML = rs.length ? rs.slice(0, 8).map((r) => resourceItem(r)).join('') : '<div class="empty">暂无资料</div>';
  }).catch((e) => { $(id).innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; });
}
