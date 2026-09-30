import { api, esc, go, layout, meta, nodeCard, qs, resourceItem, SITE_NAME, tree, $ } from '../app.js';

layout('search');

const q = (qs.get('q') || '').trim();
const type = qs.get('type') === 'resource' ? 'resource' : 'course';
const tag = type === 'resource' ? qs.get('tag') || '' : '';
const within = qs.get('within') || '';
// 本科 (also untagged courses) / 研究生, see STUDY_LEVELS.
const level = ['ug', 'grad'].includes(qs.get('level')) ? qs.get('level') : '';
const page = Math.min(50, Math.max(1, Number(qs.get('page')) || 1));

function url(p) {
  const u = new URLSearchParams({ q, type, tag, within, level, page: 1, ...p });
  for (const [k, v] of [...u]) if (!v || (k === 'page' && v === '1')) u.delete(k);
  return `/search?${u}`;
}

document.title = q ? `${q} · 搜索 · ${SITE_NAME}` : `搜索 · ${SITE_NAME}`;
$('#title').textContent = q ? `“${q}”的搜索结果` : '搜索';
$('#sq').value = q;
$('#stype').value = type;
$('#sq').placeholder = type === 'course' ? '搜索课程名称或拼音首字母' : '搜索资料名称、年份或拼音首字母';
$('#types').innerHTML = [['course', '课程'], ['resource', '资料']]
  .map(([k, l]) => `<a class="chip${k === type ? ' on' : ''}" href="${url({ type: k, tag: k === 'course' ? '' : tag })}">${l}</a>`).join('')
  + '<span class="chip-sep"></span>'
  + [['', '本研全部'], ['ug', '本科'], ['grad', '研究生']]
    .map(([k, l]) => `<a class="chip${k === level ? ' on' : ''}" href="${url({ level: k })}">${l}</a>`).join('');
$('#tag').hidden = type === 'course';

if (type === 'resource') meta().then((m) => {
  const sel = $('#tag');
  sel.innerHTML += m.tags.map((t) => `<option value="${t.code}"${t.code === tag ? ' selected' : ''}>${t.code} ${t.label}</option>`).join('');
  sel.onchange = () => { go(url({ tag: sel.value })); };
});
tree().then((t) => {
  const sel = $('#within');
  const opts = t.children(0).map((s) => `<option value="${s.id}"${String(s.id) === within ? ' selected' : ''}>${esc(s.name)}</option>`);
  // A scope picked on a course page isn't a top-level section; keep it selectable.
  const cur = within && t.byId.get(Number(within));
  if (cur && cur.parent) opts.unshift(`<option value="${cur.id}" selected>${esc(cur.name)}</option>`);
  sel.innerHTML += opts.join('');
  sel.onchange = () => { go(url({ within: sel.value })); };
  if (cur) $('#scope').innerHTML = `只在「${esc(cur.name)}」里搜索 · <a href="${url({ within: '' })}">搜全站</a>`;
});

const empty = (msg) => `<div class="empty">${msg}</div>`;
/** Nothing found: the site may simply not have it yet — offer to fill the gap. */
const missing = (what) => empty(`<b>没有找到「${esc(q)}」相关的${what}</b>换个关键词、课程简称或拼音首字母试试；如果本站确实还没有：
  <div class="row" style="justify-content:center;margin-top:10px"><a class="btn primary sm" href="/upload">我有这门课的资料，去上传</a><a class="btn sm" href="/feedback?want=${encodeURIComponent(q)}">告诉我们缺这门课</a></div>
  <p class="small faint" style="margin-top:10px">看看<a href="/missing">大家还在找哪些课</a></p>`);

function pager(total, size) {
  const pages = Math.min(50, Math.ceil(total / size));
  if (pages <= 1) return '';
  return (page > 1 ? `<a class="btn sm" href="${url({ page: page - 1 })}">上一页</a>` : '') +
    `<form class="jump small muted" id="jump">第 <input class="input" name="p" type="number" min="1" max="${pages}" value="${page}" aria-label="页码"> / ${pages} 页 <button class="btn sm" type="submit">跳转</button></form>` +
    (page < pages ? `<a class="btn sm" href="${url({ page: page + 1 })}">下一页</a>` : '');
}

// 跳转: any page from 1 to the last one.
$('#pager').addEventListener('submit', (e) => {
  e.preventDefault();
  const box = e.target.elements.p;
  const n = Math.round(Number(box.value));
  if (!n || n < 1 || n > Number(box.max)) { box.value = page; box.select(); return; }
  if (n !== page) go(url({ page: n }));
});

async function run() {
  const box = $('#results');
  if (!q) {
    box.innerHTML = empty(`<b>输入关键词开始搜索</b>按${type === 'course' ? '课程名称' : '资料名称'}查找，支持拼音首字母`);
    return;
  }
  box.innerHTML = '<div class="skeleton"></div>';
  const params = new URLSearchParams({ q, type, name_only: 'true', page });
  if (within) params.set('within', within);
  if (level) params.set('level', level);
  if (tag) params.set('tag', tag);
  try {
    const results = await api(`/search?${params}`);
    box.innerHTML = type === 'course'
      ? `<section class="results-nodes"><h2>课程 <span class="small faint">${results.total}</span></h2>
          ${results.items.length ? `<div class="grid">${results.items.map((i) => nodeCard(i.node, i.path)).join('')}</div>` : missing('课程')}</section>`
      : `<section class="card results-res"><h2>资料 <span class="small faint">${results.total}</span></h2>
          <div class="list">${results.items.length ? results.items.map((i) => resourceItem(i.resource)).join('') : missing('资料')}</div></section>`;
    $('#summary').textContent = `共 ${results.total} 条结果`;
    $('#pager').innerHTML = pager(results.total, results.page_size);
  } catch (e) {
    box.innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
  }
}
run();
