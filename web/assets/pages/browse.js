import { esc, layout, qs, store, tree, $ } from '../app.js';

layout('browse');

// ?pick=1 turns the tree into a picker for the upload page.
const picking = qs.get('pick') === '1';
const OPEN_KEY = 'xmuhub.tree.open';
let open = new Set(JSON.parse(store.get(OPEN_KEY) || '[]'));
let filter = '';

function save() { store.set(OPEN_KEY, JSON.stringify([...open])); }

function render(t) {
  // While filtering: show matches plus their ancestors, all expanded.
  let shown = null;
  if (filter) {
    shown = new Set();
    const f = filter.toLowerCase();
    for (const n of t.list) {
      if (![n.name, n.label, n.code, ...(n.aliases || [])].some((s) => s && s.toLowerCase().includes(f))) continue;
      for (let x = n; x; x = t.byId.get(x.parent)) shown.add(x.id);
    }
  }
  const li = (n) => {
    const kids = t.children(n.id).filter((k) => !shown || shown.has(k.id));
    const isOpen = shown ? true : open.has(n.id);
    // In the picker a name with children expands it; only leaves (and "选这里") pick.
    const href = picking ? (kids.length ? '#' : `/upload?node=${n.id}`) : `/n/${n.id}`;
    return `<li>
      <div class="row-n${kids.length ? ' has-kids' : ''}" ${kids.length ? `data-t="${n.id}"` : ''}>
        ${kids.length ? `<button class="tw" data-t="${n.id}" aria-label="${isOpen ? '收起' : '展开'}">${isOpen ? '▾' : '▸'}</button>` : '<span class="tw"></span>'}
        <a class="n${n.count ? '' : ' zero'}" href="${href}"${picking && kids.length ? ` data-t="${n.id}"` : ''}>${esc(n.name)}</a>
        ${n.status === 'pending' ? '<span class="badge pending">待确认</span>' : ''}
        ${picking && kids.length ? `<span class="small faint">${kids.length} 个下级</span>` : ''}
        ${picking && n.kind !== 'section' ? `<a class="btn sm pick${kids.length ? '' : ' primary'}" href="/upload?node=${n.id}">选这里</a>` : ''}
        <span class="c">${n.count || ''}</span>
      </div>
      ${kids.length && isOpen ? `<ul>${kids.map(li).join('')}</ul>` : ''}
    </li>`;
  };
  const roots = t.children(0).filter((n) => !shown || shown.has(n.id));
  $('#tree').innerHTML = roots.map(li).join('') || '<li class="empty">没有匹配的分类</li>';
}

tree().then((t) => {
  if (!store.get(OPEN_KEY)) open = new Set(t.children(0).map((n) => n.id));
  render(t);
  $('#tree').onclick = (e) => {
    // Clicking the arrow, the row background, or (in the picker) a parent's name toggles it.
    if (e.target.closest('.pick')) return;
    const name = e.target.closest('a.n');
    if (name && !name.dataset.t) return;
    const b = name || e.target.closest('[data-t]');
    if (!b) return;
    e.preventDefault();
    if (filter) return;
    const id = Number(b.dataset.t);
    open.has(id) ? open.delete(id) : open.add(id);
    save();
    render(t);
  };
  $('#expand').onclick = () => { open = new Set(t.list.map((n) => n.id)); save(); render(t); };
  $('#collapse').onclick = () => { open = new Set(); save(); render(t); };
  $('#tf').oninput = () => { filter = $('#tf').value.trim(); render(t); };
});

if (picking) {
  document.querySelector('h1').textContent = '选择上传到哪个分类';
  document.querySelector('.page-head .muted').textContent = '点学院或分组展开，选到具体课程；找不到课程可以先选学院，或回上传页新建课程。';
}
