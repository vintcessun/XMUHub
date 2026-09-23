import { esc, layout, qs, store, tree, $ } from '../app.js';

layout('browse');

// ?pick=1 turns the tree into a picker for the upload page.
const picking = qs.get('pick') === '1';
const OPEN_KEY = 'xmuhub.tree.open';
let open = new Set(JSON.parse(store.get(OPEN_KEY) || '[]'));

function render(t) {
  const li = (n) => {
    const kids = t.children(n.id);
    const isOpen = open.has(n.id);
    return `<li>
      <div class="row-n">
        ${kids.length ? `<button class="tw" data-t="${n.id}" aria-label="展开">${isOpen ? '▾' : '▸'}</button>` : '<span class="tw"></span>'}
        <a class="n${n.count ? '' : ' zero'}" href="/n/${n.id}">${esc(n.name)}</a>
        ${n.status === 'pending' ? '<span class="badge pending">待确认</span>' : ''}
        ${picking && n.kind !== 'section' ? `<a class="btn sm pick" href="/upload?node=${n.id}">选这里</a>` : ''}
        <span class="c">${n.count || ''}</span>
      </div>
      ${kids.length && isOpen ? `<ul>${kids.map(li).join('')}</ul>` : ''}
    </li>`;
  };
  $('#tree').innerHTML = t.children(0).map(li).join('');
}

tree().then((t) => {
  if (!store.get(OPEN_KEY)) open = new Set(t.children(0).map((n) => n.id));
  render(t);
  $('#tree').onclick = (e) => {
    const b = e.target.closest('[data-t]');
    if (!b) return;
    const id = Number(b.dataset.t);
    open.has(id) ? open.delete(id) : open.add(id);
    store.set(OPEN_KEY, JSON.stringify([...open]));
    render(t);
  };
  $('#expand').onclick = () => { open = new Set(t.list.map((n) => n.id)); store.set(OPEN_KEY, JSON.stringify([...open])); render(t); };
  $('#collapse').onclick = () => { open = new Set(); store.set(OPEN_KEY, '[]'); render(t); };
});

if (picking) document.querySelector('h1').textContent = '选择上传到哪个分类';
