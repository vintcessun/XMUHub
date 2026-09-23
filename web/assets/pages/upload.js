import { api, esc, fmtSize, layout, meta, qs, token, toast, $ } from '../app.js';

const mePromise = layout('upload');

const state = {
  file: null,
  parts: null, // [{start, end, size, sha256}]
  course: null, // picked {id, name, ...}
  newCourse: false,
  hashing: null,
};

// ---------------------------------------------------------------- gate

async function gate() {
  const me = await mePromise;
  const m = await meta();
  $('#droptip').textContent = `单个文件最大 ${fmtSize(m.limits.max_file)}，大文件会自动分卷上传`;
  $('#kind').innerHTML = m.kinds.map((k) => `<option value="${k.key}">${k.label}</option>`).join('');
  const y = new Date().getFullYear();
  $('#year').innerHTML += Array.from({ length: 16 }, (_, i) => `<option>${y - i}</option>`).join('');
  if (me.level >= 1) {
    $('#form').hidden = false;
    step(0);
    return;
  }
  $('#gate').innerHTML = `<section class="card">
    <h2>上传前先领取一个令牌</h2>
    <p class="muted">XMUHub 不用注册。令牌保存在你的浏览器里，用来识别你的上传；新令牌是<b>贡献者</b>级别，上传的资料会在审核通过后公开。</p>
    <div class="row"><button class="btn primary" id="claim">领取令牌并继续</button><a href="/me">我已经有令牌了</a></div>
  </section>`;
  $('#claim').onclick = async () => {
    try {
      const r = await api('/token/claim', { method: 'POST' });
      token.set(r.token);
      toast('令牌已保存到本浏览器，可以在「我的」页面备份');
      location.reload();
    } catch (e) { toast(e.message, true); }
  };
}

function step(i) {
  document.querySelectorAll('#steps span').forEach((s, j) => {
    s.className = j < i ? 'done' : j === i ? 'on' : '';
  });
}

// ---------------------------------------------------------------- file & hashing

let hasherLoaded = null;
function loadHasher() {
  if (!hasherLoaded) {
    hasherLoaded = new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = '/vendor/sha256.umd.min.js';
      s.onload = () => resolve(window.hashwasm);
      s.onerror = () => reject(new Error('加载校验组件失败'));
      document.head.appendChild(s);
    });
  }
  return hasherLoaded;
}

async function hashFile(file, maxPart, onProgress) {
  const hw = await loadHasher();
  const parts = [];
  const CHUNK = 4 * 1024 * 1024;
  let done = 0;
  for (let start = 0; start < file.size; start += maxPart) {
    const end = Math.min(file.size, start + maxPart);
    const h = await hw.createSHA256();
    h.init();
    for (let off = start; off < end; off += CHUNK) {
      const buf = new Uint8Array(await file.slice(off, Math.min(end, off + CHUNK)).arrayBuffer());
      h.update(buf);
      done += buf.length;
      onProgress(done / file.size);
    }
    parts.push({ start, end, size: end - start, sha256: h.digest('hex') });
  }
  return parts;
}

async function pickFile(file) {
  const m = await meta();
  if (!file) return;
  if (file.size === 0) return toast('这是一个空文件', true);
  if (file.size > m.limits.max_file) return toast(`文件太大，最大 ${fmtSize(m.limits.max_file)}`, true);
  state.file = file;
  state.parts = null;
  const info = $('#fileinfo');
  info.hidden = false;
  info.innerHTML = `<div class="row"><b style="overflow-wrap:anywhere">${esc(file.name)}</b><span class="muted small">${fmtSize(file.size)}</span></div>
    <div class="small muted" id="hashmsg">正在计算校验值…</div><div class="progress"><i id="hashbar"></i></div>`;
  if (!$('#title').value) $('#title').value = file.name.replace(/\.[^.]+$/, '');
  step(1);
  const my = (state.hashing = Symbol());
  try {
    const parts = await hashFile(file, m.limits.max_part, (p) => { if (state.hashing === my) $('#hashbar').style.width = `${Math.round(p * 100)}%`; });
    if (state.hashing !== my) return;
    state.parts = parts;
    $('#hashmsg').textContent = parts.length > 1 ? `校验完成，将分 ${parts.length} 卷上传` : '校验完成';
  } catch (e) {
    $('#hashmsg').textContent = e.message;
  }
}

$('#file').onchange = (e) => pickFile(e.target.files[0]);
const drop = $('#drop');
drop.ondragover = (e) => { e.preventDefault(); drop.classList.add('over'); };
drop.ondragleave = () => drop.classList.remove('over');
drop.ondrop = (e) => { e.preventDefault(); drop.classList.remove('over'); pickFile(e.dataTransfer.files[0]); };

// ---------------------------------------------------------------- course picker

function showPicked() {
  const c = state.course;
  $('#picked').hidden = !c;
  $('#coursepick').hidden = !!c || state.newCourse;
  $('#coursenew').hidden = !state.newCourse;
  if (c) {
    $('#picked').innerHTML = `<b>${esc(c.name)}</b><span class="small muted">${esc(c.code || '')} ${esc(c.college || '')}</span><span class="grow"></span><a href="#" id="unpick">更换</a>`;
    $('#unpick').onclick = (e) => { e.preventDefault(); state.course = null; showPicked(); $('#cq').focus(); };
  }
}

let timer = 0;
let seq = 0;
$('#cq').oninput = () => {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const q = $('#cq').value.trim();
    const ul = $('#cs');
    if (!q) { ul.hidden = true; return; }
    const my = ++seq;
    const list = await api(`/courses/suggest?q=${encodeURIComponent(q)}`).catch(() => []);
    if (my !== seq) return;
    ul.hidden = false;
    ul.innerHTML = list.length
      ? list.map((c, i) => `<li data-i="${i}">${esc(c.name)}<small>${esc(c.code)} ${esc(c.college)}${c.status === 'pending' ? ' · 待确认' : ''}</small></li>`).join('')
      : `<li data-new="1">没有找到「${esc(q)}」，点这里新建课程</li>`;
    ul.onclick = (e) => {
      const li = e.target.closest('li');
      if (!li) return;
      ul.hidden = true;
      if (li.dataset.new) {
        state.newCourse = true;
        $('#nc_name').value = q;
      } else {
        state.course = list[Number(li.dataset.i)];
      }
      showPicked();
    };
  }, 180);
};
document.addEventListener('click', (e) => { if (!e.target.closest('.suggest')) $('#cs').hidden = true; });
$('#newcourse').onclick = (e) => { e.preventDefault(); state.newCourse = true; $('#nc_name').value = $('#cq').value.trim(); showPicked(); };
$('#backpick').onclick = (e) => { e.preventDefault(); state.newCourse = false; showPicked(); };

if (qs.get('course')) {
  api(`/courses/${Number(qs.get('course'))}`).then((d) => { state.course = d.course; showPicked(); }).catch(() => {});
}

// ---------------------------------------------------------------- upload

function sendPart(target, blob, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open(target.method, target.url);
    for (const [k, v] of target.headers) xhr.setRequestHeader(k, v);
    xhr.upload.onprogress = (e) => onProgress(e.loaded);
    xhr.onload = () => {
      let body = null;
      try { body = JSON.parse(xhr.responseText); } catch { /* ignore */ }
      if (xhr.status >= 200 && xhr.status < 300) resolve(body || {});
      else reject(new Error((body && (body.error || body.message)) || `上传失败（HTTP ${xhr.status}）`));
    };
    xhr.onerror = () => reject(new Error('网络中断'));
    xhr.send(blob);
  });
}

function setStatus(html, cls = '') {
  $('#status').innerHTML = html ? `<div class="notice ${cls}">${html}</div>` : '';
}

$('#form').onsubmit = async (e) => {
  e.preventDefault();
  const f = state.file;
  if (!f) return toast('请先选择文件', true);
  if (!state.parts) return toast('文件还在校验中，请稍候', true);
  if (!state.course && !state.newCourse) return toast('请选择课程', true);
  if (state.newCourse && $('#nc_name').value.trim().length < 2) return toast('请填写课程名称', true);
  if ($('#title').value.trim().length < 2) return toast('请填写标题', true);

  const btn = $('#submit');
  btn.disabled = true;
  const bar = $('#prog');
  bar.hidden = false;
  step(2);
  try {
    setStatus('正在准备上传…');
    let plan = await api('/uploads', {
      method: 'POST',
      body: { filename: f.name, mime: f.type, parts: state.parts.map((p) => ({ size: p.size, sha256: p.sha256 })) },
    });
    if (plan.dedup) {
      setStatus('服务器上已经有完全相同的文件，秒传完成 ✓', 'ok');
    } else {
      let sent = 0;
      for (const pp of plan.parts) {
        if (pp.done) { sent += pp.size; continue; }
        const part = state.parts[pp.index];
        let attempt = 0;
        for (;;) {
          try {
            setStatus(plan.parts.length > 1 ? `正在上传第 ${pp.index + 1} / ${plan.parts.length} 卷…` : '正在上传…');
            const target = attempt === 0 ? pp.target : (await api(`/uploads/${plan.upload_id}/parts/${pp.index}/renew`, { method: 'POST' })).target;
            const receipt = await sendPart(target, f.slice(part.start, part.end), (n) => {
              bar.firstElementChild.style.width = `${Math.round(((sent + n) / f.size) * 100)}%`;
            });
            setStatus('正在校验…');
            await api(`/uploads/${plan.upload_id}/parts/${pp.index}`, { method: 'POST', body: { asset_id: receipt.id ?? null } });
            break;
          } catch (err) {
            if (++attempt >= 3) throw err;
            setStatus(`${esc(err.message)}，正在重试（${attempt}/2）…`, 'warn');
            await new Promise((r) => setTimeout(r, 1500 * attempt));
          }
        }
        sent += pp.size;
      }
    }
    bar.firstElementChild.style.width = '100%';
    setStatus('正在提交资料信息…');
    const body = {
      upload_id: plan.upload_id,
      title: $('#title').value, kind: $('#kind').value,
      year: Number($('#year').value) || null, term: Number($('#term').value) || null,
      teacher: $('#teacher').value, description: $('#description').value,
    };
    if (state.course) body.course_id = state.course.id;
    else body.new_course = { name: $('#nc_name').value, code: $('#nc_code').value, college: $('#nc_college').value };
    const r = await api('/resources', { method: 'POST', body });
    step(4);
    $('#form').hidden = true;
    const done = $('#done');
    done.hidden = false;
    done.innerHTML = r.status === 'published'
      ? `<h2>上传成功 🎉</h2><p>资料已经公开，谢谢你的分享！</p><div class="row"><a class="btn primary" href="/r/${r.id}">查看资料</a><a class="btn" href="/upload?course=${r.course.id}">继续上传这门课</a></div>`
      : `<h2>上传成功，等待审核</h2><p class="muted">审核通过后所有人都能看到它。你可以在「我的」页面查看审核进度。</p><div class="row"><a class="btn primary" href="/r/${r.id}">查看资料</a><a class="btn" href="/upload?course=${r.course.id}">继续上传这门课</a></div>`;
  } catch (err) {
    step(1);
    setStatus(esc(err.message), 'bad');
    btn.disabled = false;
  }
};

gate();
