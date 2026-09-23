import { api, courseCard, esc, layout, qs, $ } from '../app.js';

layout('courses');

let all = [];
let college = qs.get('college') || '';

function render() {
  const f = $('#filter').value.trim().toLowerCase();
  const list = all.filter((c) =>
    (!college || c.college === college) &&
    (!f || c.name.toLowerCase().includes(f) || c.code.toLowerCase().includes(f) || c.aliases.some((a) => a.toLowerCase().includes(f))));
  $('#summary').textContent = `${list.length} 门课程`;
  $('#list').innerHTML = list.length ? list.map(courseCard).join('') : '<div class="empty card" style="grid-column:1/-1"><b>没有匹配的课程</b></div>';
}

function renderColleges(cols) {
  $('#colleges').innerHTML = [['', '全部学院'], ...cols.map((c) => [c.name, `${c.name} ${c.count}`])]
    .map(([k, l]) => `<button class="chip${k === college ? ' on' : ''}" data-c="${esc(k)}">${esc(l)}</button>`).join('');
  $('#colleges').onclick = (e) => {
    const b = e.target.closest('[data-c]');
    if (!b) return;
    college = b.dataset.c;
    history.replaceState(null, '', college ? `?college=${encodeURIComponent(college)}` : location.pathname);
    renderColleges(cols);
    render();
  };
}

Promise.all([api('/courses'), api('/colleges')]).then(([cs, cols]) => {
  all = cs;
  renderColleges(cols);
  render();
}).catch((e) => { $('#list').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; });

$('#filter').oninput = render;
