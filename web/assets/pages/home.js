import { api, esc, layout, meta, resourceItem, tree, $ } from '../app.js';

layout('home');

meta().then((m) => {
  const s = m.stats;
  $('#stats').innerHTML = `<span><b>${s.resources}</b>份资料</span><span><b>${s.nodes}</b>门有资料的课程</span><span><b>${s.downloads}</b>次下载</span>`;
}).catch(() => {});

tree().then((t) => {
  const sections = t.children(0);
  $('#sections').innerHTML = sections.map((s) => {
    const groups = t.children(s.id);
    return `<section class="card section-card">
      <h3><a href="/n/${s.id}">${esc(s.name)}</a><span class="small faint">${s.count} 份</span></h3>
      <div class="chips">${groups.map((g) => `<a class="chip" href="/n/${g.id}" title="${esc(g.name)}">${esc(g.name.replace(/^[A-D]\d+-/, ''))}${g.count ? ` <b>${g.count}</b>` : ''}</a>`).join('')}</div>
    </section>`;
  }).join('') || '<div class="empty card"><b>分类还没有建立</b></div>';
}).catch((e) => { $('#sections').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; });

for (const [id, path] of [['#recent', '/recent'], ['#popular', '/popular']]) {
  api(path).then((rs) => {
    $(id).innerHTML = rs.length ? rs.slice(0, 8).map((r) => resourceItem(r)).join('') : '<div class="empty">暂无资料</div>';
  }).catch((e) => { $(id).innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; });
}
