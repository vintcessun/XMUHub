import { ago, api, esc, layout, $ } from '../app.js';

// 缺资料的课程: searches that found nothing, as picked by admins (「搜不到」 on the admin page).
// Each links to the upload page and to the search itself (which may find something by now).

layout('missing');

(async () => {
  let list;
  try { list = await api('/missing'); } catch (e) { $('#mlist').innerHTML = `<div class="notice bad">${esc(e.message)}</div>`; return; }
  $('#mlist').innerHTML = list.length
    ? list.map((m) => `<div class="item"><div class="body"><b>${esc(m.term)}</b>
        <div class="meta"><span>${m.count} 人次搜索</span><span>最近 ${ago(m.last)}</span></div></div>
        <a class="btn sm" href="/search?q=${encodeURIComponent(m.term)}">搜索看看</a><a class="btn sm primary" href="/upload">我有资料，去上传</a></div>`).join('')
    : '<div class="empty"><b>暂时没有</b>大家搜的课程本站都有资料了</div>';
})();
