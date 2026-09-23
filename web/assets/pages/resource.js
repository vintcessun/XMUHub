import { api, downloadResource, esc, fmtDate, fmtSize, layout, meta, pathId, statusBadge, TERMS, toast, $ } from '../app.js';

const id = pathId();
const mePromise = layout('courses');

async function load() {
  let r;
  try {
    r = await api(`/resources/${id}`);
  } catch (e) {
    $('#title').textContent = e.status === 404 ? '资料不存在或尚未通过审核' : '加载失败';
    $('#dl').disabled = true;
    return;
  }
  document.title = `${r.title} · XMUHub`;
  $('#crumbs').innerHTML = `<a href="/courses">课程</a> / <a href="/c/${r.course.id}">${esc(r.course.name)}</a>`;
  $('#title').textContent = r.title;
  $('#tags').innerHTML = `<span class="tag ${esc(r.kind)}">${esc(r.kind_label)}</span>${statusBadge(r.status, r.needs_review)}<span>${r.downloads} 次下载</span>`;
  $('#desc').textContent = r.description || '';
  $('#desc').hidden = !r.description;
  const rows = [
    ['课程', `<a href="/c/${r.course.id}">${esc(r.course.name)}</a>${r.course.code ? ` <span class="mono faint">${esc(r.course.code)}</span>` : ''}`],
    ['年份', r.year ? `${r.year}${r.term ? ' ' + TERMS[r.term] + '学期' : ''}` : ''],
    ['老师', esc(r.teacher)],
    ['文件名', esc(r.filename)],
    ['大小', fmtSize(r.size)],
    ['上传于', fmtDate(r.created_at)],
  ].filter(([, v]) => v);
  $('#kv').innerHTML = rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('');
  const notes = {
    pending: ['warn', '这份资料正在等待审核，通过后才会对所有人可见。'],
    rejected: ['bad', `这份资料未通过审核。${r.review_note ? '原因：' + esc(r.review_note) : ''}`],
    removed: ['bad', `这份资料已下架。${r.review_note ? '原因：' + esc(r.review_note) : ''}`],
  };
  const n = notes[r.status];
  $('#notice').innerHTML = n ? `<div class="notice ${n[0]}">${n[1]}</div>` : '';
  $('#dl').disabled = r.status === 'rejected' || r.status === 'removed';
  $('#dl').textContent = `下载 · ${fmtSize(r.size)}`;

  const me = await mePromise;
  if (me.level >= 3 || (r.mine && r.status === 'pending')) renderManage(r, me);
}

$('#dl').onclick = async () => {
  const btn = $('#dl');
  const bar = $('#dlprog');
  btn.disabled = true;
  bar.hidden = false;
  const label = btn.textContent;
  btn.textContent = '正在连接镜像…';
  try {
    const res = await downloadResource(id, (p) => {
      bar.firstElementChild.style.width = `${Math.round(p * 100)}%`;
      btn.textContent = `下载中 ${Math.round(p * 100)}%`;
    });
    if (res.ok) {
      btn.textContent = res.fallback ? '已在新连接中开始下载' : '下载完成';
    } else {
      btn.textContent = label;
      $('#parts').hidden = false;
      $('#parts').innerHTML = `<p class="notice warn" style="margin-top:12px">自动合并失败，请依次下载以下分卷后按顺序合并：</p>` +
        res.plan.parts.map((p, i) => `<div><a href="${esc(p.urls[0])}" rel="noreferrer">分卷 ${i + 1}</a> · ${fmtSize(p.size)}</div>`).join('');
    }
  } catch (e) {
    toast(e.message, true);
    btn.textContent = label;
  } finally {
    setTimeout(() => { btn.disabled = false; bar.hidden = true; if (btn.textContent.startsWith('下载完成') || btn.textContent.startsWith('已在')) btn.textContent = label; }, 4000);
  }
};

async function renderManage(r, me) {
  const box = $('#manage');
  box.hidden = false;
  const m = await meta();
  const reviewer = me.level >= 3;
  const actions = reviewer
    ? [
        (r.status !== 'published' || r.needs_review) && ['approve', '通过', 'ok'],
        r.status === 'pending' && ['reject', '驳回', 'danger'],
        r.status === 'published' && ['remove', '下架', 'danger'],
        (r.status === 'removed' || r.status === 'rejected') && ['restore', '恢复发布', ''],
      ].filter(Boolean)
    : [];
  box.innerHTML = `<h3>${reviewer ? '审核' : '修改我的投稿'}</h3>
    ${actions.length ? `<label class="field"><span>备注（驳回/下架原因）</span><input class="input" id="note"></label>
      <div class="row" style="margin-bottom:16px">${actions.map(([a, l, c]) => `<button class="btn sm ${c}" data-a="${a}">${l}</button>`).join('')}</div>` : ''}
    <details><summary class="small">编辑信息</summary><div style="margin-top:12px">
      <label class="field"><span>标题</span><input class="input" id="f_title" value="${esc(r.title)}"></label>
      <label class="field"><span>类型</span><select class="input" id="f_kind">${m.kinds.map((k) => `<option value="${k.key}"${k.key === r.kind ? ' selected' : ''}>${k.label}</option>`).join('')}</select></label>
      <div class="fields-2">
        <label class="field"><span>年份</span><input class="input" id="f_year" type="number" value="${r.year || ''}"></label>
        <label class="field"><span>学期</span><select class="input" id="f_term"><option value="">不确定</option>${[1, 2, 3].map((t) => `<option value="${t}"${t === r.term ? ' selected' : ''}>${TERMS[t]}</option>`).join('')}</select></label>
      </div>
      <label class="field"><span>老师</span><input class="input" id="f_teacher" value="${esc(r.teacher)}"></label>
      <label class="field"><span>说明</span><textarea class="input" id="f_desc">${esc(r.description)}</textarea></label>
      ${reviewer ? `<label class="field"><span>移到课程 ID</span><input class="input" id="f_course" value="${r.course.id}"></label>` : ''}
      <button class="btn primary sm" id="f_save">保存</button>
    </div></details>`;
  box.querySelectorAll('[data-a]').forEach((b) => {
    b.onclick = async () => {
      try {
        const res = await api(`/resources/${id}/review`, { method: 'POST', body: { action: b.dataset.a, note: $('#note').value } });
        toast(`已处理：${res.status}`);
        load();
      } catch (e) { toast(e.message, true); }
    };
  });
  $('#f_save').onclick = async () => {
    try {
      await api(`/resources/${id}`, {
        method: 'PATCH',
        body: {
          title: $('#f_title').value, kind: $('#f_kind').value,
          year: Number($('#f_year').value) || null, term: Number($('#f_term').value) || null,
          teacher: $('#f_teacher').value, description: $('#f_desc').value,
          course_id: reviewer ? Number($('#f_course').value) || null : null,
        },
      });
      toast('已保存');
      load();
    } catch (e) { toast(e.message, true); }
  };
}

load();
