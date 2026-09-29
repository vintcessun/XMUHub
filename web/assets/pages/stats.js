import { api, esc, fmtSize, layout, $ } from '../app.js';

// 统计: public numbers, ranked by course only (never by person).

function rank(title, list, unit) {
  return `<section class="card"><h3>${title}</h3>${list.length
    ? `<ol class="rank">${list.map((c) => `<li><a href="/n/${c.id}">${esc(c.name)}</a><span class="small faint">${esc(c.path)}</span><span class="v">${c.value.toLocaleString('zh-CN')} ${unit}</span></li>`).join('')}</ol>`
    : '<p class="small faint">暂无</p>'}</section>`;
}

(async () => {
  await layout('stats');
  let s;
  try { s = await api('/stats'); } catch (e) { $('#stats').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  const max = Math.max(1, ...s.weekly.map(([, n]) => n));
  const week = (t) => { const d = new Date(t * 1000); return `${d.getMonth() + 1}/${d.getDate()}`; };
  const n = (x) => x.toLocaleString('zh-CN');
  $('#stats').innerHTML = `<section class="card"><div class="statnums">
      <div><b>${n(s.files)}</b><span class="small muted">份公开资料</span></div>
      <div><b>${n(s.courses)}</b><span class="small muted">门课程有资料</span></div>
      <div><b>${n(s.downloads)}</b><span class="small muted">次下载</span></div>
      <div><b>${fmtSize(s.bytes)}</b><span class="small muted">资料总大小</span></div>
      <div><b>${n(s.users)}</b><span class="small muted">位注册同学</span></div></div></section>
    <section class="card"><h3>每周新增资料（近半年）</h3>
      <div class="weekbars" role="img" aria-label="每周新增资料数">${s.weekly.map(([t, c]) => `<i style="height:${Math.max(2, Math.round((c / max) * 100))}%" title="${week(t)} 起的一周：${c} 份"></i>`).join('')}</div>
      <div class="row small faint" style="justify-content:space-between;margin-top:6px"><span>${week(s.weekly[0][0])}</span><span>本周 ${s.weekly[s.weekly.length - 1][1]} 份</span></div></section>
    <div class="grid" style="grid-template-columns:repeat(auto-fit,minmax(300px,1fr))">
      ${rank('下载最多的课程', s.most_downloaded, '次')}
      ${rank('资料最多的课程', s.most_files, '份')}
      ${rank('近 30 天新增最多', s.recently_added, '份')}
    </div>`;
})();
