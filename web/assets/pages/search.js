import { api, courseCard, esc, layout, meta, qs, resourceItem, $ } from '../app.js';

layout('search');

const q = (qs.get('q') || '').trim();
const type = qs.get('type') || '';
const kind = qs.get('kind') || '';
const page = Math.max(1, Number(qs.get('page')) || 1);

function url(p) {
  const u = new URLSearchParams({ q, type, kind, page: 1, ...p });
  for (const [k, v] of [...u]) if (!v) u.delete(k);
  return `/search?${u}`;
}

document.title = q ? `${q} · 搜索 · XMUHub` : '搜索 · XMUHub';
$('#title').textContent = q ? `“${q}”的搜索结果` : '搜索';
$('#types').innerHTML = [['', '全部'], ['course', '课程'], ['resource', '资料']]
  .map(([k, l]) => `<a class="chip${k === type ? ' on' : ''}" href="${url({ type: k })}">${l}</a>`).join('');

meta().then((m) => {
  const sel = $('#kind');
  sel.innerHTML += m.kinds.map((k) => `<option value="${k.key}"${k.key === kind ? ' selected' : ''}>${k.label}</option>`).join('');
  sel.onchange = () => { location.href = url({ kind: sel.value, type: sel.value ? 'resource' : type }); };
});

async function run() {
  const box = $('#results');
  if (!q) {
    box.innerHTML = '<div class="empty"><b>输入关键词开始搜索</b>支持课程名、课程代码、老师，以及拼音首字母（如 gdsx → 高等数学）</div>';
    return;
  }
  box.innerHTML = '<div class="skeleton"></div>';
  try {
    const r = await api(`/search?${new URLSearchParams({ q, type, kind, page })}`);
    $('#summary').textContent = `共 ${r.total} 条结果`;
    if (!r.items.length) {
      box.innerHTML = `<div class="empty"><b>没有找到相关内容</b>换个关键词试试，或者 <a href="/upload">上传这份资料</a></div>`;
      return;
    }
    const courses = r.items.filter((i) => i.type === 'course');
    const res = r.items.filter((i) => i.type === 'resource');
    box.innerHTML =
      (courses.length ? `<div class="grid" style="margin-bottom:${res.length ? 12 : 0}px">${courses.map((i) => courseCard(i.course)).join('')}</div>` : '') +
      res.map((i) => resourceItem(i.resource)).join('');
    const pages = Math.ceil(r.total / r.page_size);
    if (pages > 1) {
      $('#pager').innerHTML =
        (page > 1 ? `<a class="btn sm" href="${url({ page: page - 1 })}">上一页</a>` : '') +
        `<span class="small muted">第 ${page} / ${pages} 页</span>` +
        (page < pages ? `<a class="btn sm" href="${url({ page: page + 1 })}">下一页</a>` : '');
    }
  } catch (e) {
    box.innerHTML = `<div class="notice bad">${esc(e.message)}</div>`;
  }
}
run();
