import { api, esc, layout, loginUrl, meta, resourceItem, SLOGAN, tree, $ } from '../app.js';

const user = await layout('home');
$('#slogan').textContent = SLOGAN;

meta().then((m) => {
  const s = m.stats;
  $('#stats').innerHTML = `<span><b>${s.resources}</b>份资料</span><span><b>${s.nodes}</b>门有资料的课程</span><span><b>${s.downloads}</b>次下载</span>`;
}).catch(() => {});

if (!user) {
  $('#sections').innerHTML = `<section class="card"><h2>登录后查看学习资料</h2><p class="muted">分类、搜索、预览和下载均需要登录，注册只需邮箱验证。</p><a class="btn primary" href="${loginUrl()}">登录 / 注册</a></section>`;
  $('.home-lists').hidden = true;
} else {
  tree().then((t) => {
    const sections = t.children(0);
    $('#sections').innerHTML = sections.map((s) => {
      const groups = t.children(s.id);
      return `<section class="card section-card">
        <h3><a href="/n/${s.id}">${esc(s.name)}</a><span class="small faint">${s.count} 份</span></h3>
        <div class="chips">${groups.map((g) => `<a class="chip" href="/n/${g.id}" title="${esc(g.name)}">${esc(g.name.replace(/^[A-D]\d+-/, ''))}${g.count ? ` <b>${g.count}</b>` : ''}</a>`).join('')}</div>
        <button class="more" type="button" hidden>展开全部 ${groups.length} 个 ▾</button>
      </section>`;
    }).join('') || '<div class="empty card"><b>分类还没有建立</b></div>';
    // Every card starts at the same few rows; the ones with more get a 展开 button.
    for (const card of document.querySelectorAll('#sections > .section-card')) {
      const chips = card.querySelector('.chips');
      const more = card.querySelector('.more');
      more.hidden = chips.scrollHeight <= chips.clientHeight + 2;
      card.classList.toggle('clipped', !more.hidden);
      more.onclick = () => {
        const open = card.classList.toggle('open');
        more.textContent = open ? '收起 ▴' : `展开全部 ${chips.children.length} 个 ▾`;
      };
    }
  }).catch((e) => { $('#sections').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; });

  for (const [id, path] of [['#recent', '/recent'], ['#popular', '/popular']]) {
    api(path).then((rs) => {
      $(id).innerHTML = rs.length ? rs.slice(0, 8).map((r) => resourceItem(r)).join('') : '<div class="empty">暂无资料</div>';
    }).catch((e) => { $(id).innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; });
  }
}
