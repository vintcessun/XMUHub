import { api, esc, go, layout, meta, nodeCard, qs, resourceItem, tree, $ } from '../app.js';

layout('search');

const q = (qs.get('q') || '').trim();
const type = qs.get('type') || '';
const tag = qs.get('tag') || '';
const within = qs.get('within') || '';
const page = Math.max(1, Number(qs.get('page')) || 1);

function url(p) {
  const u = new URLSearchParams({ q, type, tag, within, page: 1, ...p });
  for (const [k, v] of [...u]) if (!v || (k === 'page' && v === '1')) u.delete(k);
  return `/search?${u}`;
}

document.title = q ? `${q} · 搜索 · 鹭岛书阁` : '搜索 · 鹭岛书阁';
$('#title').textContent = q ? `“${q}”的搜索结果` : '搜索';
$('#sq').value = q;
$('#types').innerHTML = [['', '全部'], ['node', '只看课程与分类'], ['resource', '只看资料']]
  .map(([k, l]) => `<a class="chip${k === type ? ' on' : ''}" href="${url({ type: k })}">${l}</a>`).join('');

meta().then((m) => {
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

function pager(total, size) {
  const pages = Math.min(50, Math.ceil(total / size));
  if (pages <= 1) return '';
  return (page > 1 ? `<a class="btn sm" href="${url({ page: page - 1 })}">上一页</a>` : '') +
    `<span class="small muted">第 ${page} / ${pages} 页</span>` +
    (page < pages ? `<a class="btn sm" href="${url({ page: page + 1 })}">下一页</a>` : '');
}

async function run() {
  const box = $('#results');
  if (!q) {
    box.innerHTML = empty('<b>输入关键词开始搜索</b>支持课程名、代号、老师、年份，以及拼音首字母（如 wjf → 微积分，dxwl → 大学物理）');
    return;
  }
  box.innerHTML = '<div class="skeleton"></div>';
  const base = Object.fromEntries(Object.entries({ q, tag, within }).filter(([, v]) => v));
  // Courses/categories and files are ranked separately, so a course never hides behind
  // pages of its own files (and vice versa). A tag filter only applies to files.
  const wantNodes = type !== 'resource' && !tag && page === 1;
  const wantRes = type !== 'node';
  try {
    const [nodes, res] = await Promise.all([
      wantNodes || type === 'node' ? api(`/search?${new URLSearchParams({ ...base, type: 'node', page: type === 'node' ? page : 1 })}`) : null,
      wantRes ? api(`/search?${new URLSearchParams({ ...base, type: 'resource', page })}`) : null,
    ]);
    const parts = [];
    if (nodes && nodes.items.length) {
      const shown = type === 'node' ? nodes.items : nodes.items.slice(0, 6);
      parts.push(`<section class="results-nodes"><h2>课程与分类 <span class="small faint">${nodes.total}</span>
          ${type !== 'node' && nodes.total > shown.length ? `<a class="small" href="${url({ type: 'node' })}" style="margin-left:8px">查看全部</a>` : ''}</h2>
        <div class="grid">${shown.map((i) => nodeCard(i.node, i.path)).join('')}</div></section>`);
    }
    if (res) {
      parts.push(`<section class="card results-res"><h2>资料 <span class="small faint">${res.total}</span></h2>
        <div class="list">${res.items.length ? res.items.map((i) => resourceItem(i.resource)).join('') : empty(`没有找到相关资料，换个关键词试试，或者 <a href="/upload">上传这份资料</a>`)}</div></section>`);
    }
    if (!parts.length) parts.push(`<section class="card">${empty('<b>没有找到相关内容</b>换个关键词试试')}</section>`);
    box.innerHTML = parts.join('');
    const total = (nodes ? nodes.total : 0) + (res ? res.total : 0);
    $('#summary').textContent = `共 ${total} 条结果`;
    $('#pager').innerHTML = type === 'node' ? pager(nodes.total, nodes.page_size) : res ? pager(res.total, res.page_size) : '';
  } catch (e) {
    box.innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
  }
}
run();
