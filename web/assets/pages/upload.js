import { api, esc, fmtSize, layout, loginUrl, meta, pathText, qs, store, toast, tree, $ } from '../app.js';

const state = { node: null, path: [], rows: [], busy: false };
let M = null; // meta

// ---------------------------------------------------------------- draft (survives a refresh)
//
// Browsers never let a page keep the chosen files across a reload, so the draft keeps
// everything else: the category, each file's filled-in fields, its hashes and its server
// upload id. Choosing the same files again restores the fields and resumes the upload
// from the first part that hadn't finished.

const DRAFT_KEY = 'xmuhub.upload.draft';
const DRAFT_TTL = 20 * 3600 * 1000; // the server drops unfinished uploads after 24 h
const FIELDS = ['type_word', 'year', 'term', 'paper', 'with_answer', 'extra', 'note', 'subtitle'];

const fileKey = (f) => `${f.name}|${f.size}|${f.lastModified}`;

function readDraft() {
  try {
    const d = JSON.parse(store.get(DRAFT_KEY) || 'null');
    if (d && Date.now() - d.at < DRAFT_TTL) return d;
  } catch { /* corrupt */ }
  return { at: Date.now(), node: null, files: {} };
}
let draft = readDraft();

function saveDraft() {
  draft.at = Date.now();
  draft.node = state.node ? state.node.id : null;
  const files = {};
  for (const r of state.rows) {
    if (r.done) continue;
    const e = { name: r.file.name, size: r.file.size };
    for (const k of FIELDS) e[k] = r[k];
    if (r.parts) e.parts = r.parts;
    if (r.upload_id) e.upload_id = r.upload_id;
    files[fileKey(r.file)] = e;
  }
  // Files from before the refresh that haven't been chosen again yet stay in the draft.
  for (const [k, v] of Object.entries(draft.files || {})) if (!(k in files) && !state.rows.some((r) => fileKey(r.file) === k)) files[k] = v;
  draft.files = files;
  store.set(DRAFT_KEY, Object.keys(files).length || draft.node ? JSON.stringify(draft) : null);
}

function showDraftNotice() {
  const waiting = Object.entries(draft.files || {}).filter(([k]) => !state.rows.some((r) => fileKey(r.file) === k));
  const box = $('#draftnote');
  if (!waiting.length) { box.hidden = true; return; }
  box.hidden = false;
  box.innerHTML = `<div class="notice warn"><b>上次还有 ${waiting.length} 个文件没有提交</b>（页面刷新或关闭了）。重新选择这些文件，填过的信息会自动恢复，已传完的部分不用重传：
    <div class="small" style="margin-top:6px">${waiting.slice(0, 8).map(([, v]) => `${esc(v.name)} · ${fmtSize(v.size)}`).join('<br>')}${waiting.length > 8 ? `<br>… 等 ${waiting.length} 个` : ''}</div>
    <div style="margin-top:8px"><a href="#" id="draftclear" class="small">不用了，清除记录</a></div></div>`;
  $('#draftclear').onclick = (e) => { e.preventDefault(); draft.files = {}; saveDraft(); showDraftNotice(); };
}

// Warn before leaving with files that haven't been submitted.
window.addEventListener('beforeunload', (e) => {
  if (state.busy || state.rows.some((r) => !r.done)) { e.preventDefault(); e.returnValue = ''; }
});

// ---------------------------------------------------------------- gate

(async () => {
  const me = await layout('upload');
  if (!me) {
    $('#gate').innerHTML = `<section class="card"><h2>请先登录</h2><p class="muted">上传资料需要一个账号，注册只需邮箱验证码。</p>
      <a class="btn primary" href="${loginUrl()}">登录 / 注册</a></section>`;
    return;
  }
  M = await meta();
  $('#form').hidden = false;
  $('#droptip').textContent = `单个文件最大 ${fmtSize(M.limits.max_file)}，大文件会自动分卷上传`;
  $('#b_type').innerHTML += M.type_words.map((t) => `<option>${t.word}</option>`).join('');
  $('#b_year').innerHTML += yearOptions('');
  const startNode = Number(qs.get('node')) || draft.node;
  if (startNode) {
    try { const d = await api(`/nodes/${startNode}`); pick(d.node, d.path); } catch { /* ignore */ }
  }
  showDraftNotice();
  if (me.level < 2) $('#hint').textContent = '你的上传会在审核通过后公开。';
})();

function yearOptions(cur) {
  const y = new Date().getFullYear();
  const acad = Array.from({ length: 16 }, (_, i) => `${y - i}-${y - i + 1}`);
  const single = Array.from({ length: 16 }, (_, i) => `${y + 1 - i}`);
  const opt = (v) => `<option${v === cur ? ' selected' : ''}>${v}</option>`;
  // Exam months (四六级 202406) and older years aren't in the lists; keep them selectable.
  const extra = cur && !acad.includes(cur) && !single.includes(cur) ? opt(cur) : '';
  return `${extra}<optgroup label="学年">${acad.map(opt).join('')}</optgroup><optgroup label="年份">${single.map(opt).join('')}</optgroup>`;
}

// ---------------------------------------------------------------- node picker

function pick(node, path) {
  state.node = node;
  state.path = path || [];
  $('#nodepick').hidden = true;
  $('#coursenew').hidden = true;
  $('#picked').hidden = false;
  $('#picked').innerHTML = `<div><b>${esc(node.name)}</b> <span class="small muted">${esc(pathText(state.path))}</span>
    <div class="small faint">文件名将以「${esc(node.label || node.name)}」开头</div></div><span class="grow"></span><a href="#" id="unpick">更换</a>`;
  $('#unpick').onclick = (e) => { e.preventDefault(); state.node = null; saveDraft(); $('#picked').hidden = true; $('#nodepick').hidden = false; $('#nq').focus(); };
  saveDraft();
  renderRows();
}

let timer = 0;
let seq = 0;
$('#nq').oninput = () => {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const q = $('#nq').value.trim();
    const ul = $('#ns');
    if (!q) { ul.hidden = true; return; }
    const my = ++seq;
    const list = await api(`/nodes/suggest?q=${encodeURIComponent(q)}`).catch(() => []);
    if (my !== seq) return;
    ul.hidden = false;
    ul.innerHTML = list.length
      ? list.map((x, i) => `<li data-i="${i}">${esc(x.node.name)}<small>${esc(pathText(x.path))}${x.node.status === 'pending' ? ' · 待确认' : ''}</small></li>`).join('')
      : '<li class="faint">没有找到，试试拼音首字母，或在分类树里选</li>';
    ul.onclick = (e) => {
      const li = e.target.closest('li[data-i]');
      if (!li) return;
      ul.hidden = true;
      const x = list[Number(li.dataset.i)];
      pick(x.node, x.path);
    };
  }, 160);
};
document.addEventListener('click', (e) => { if (!e.target.closest('.suggest')) $('#ns').hidden = true; });

$('#newcourse').onclick = async (e) => {
  e.preventDefault();
  const t = await tree();
  const b = t.children(0).find((s) => s.code === 'B' || s.name.startsWith('B'));
  const colleges = b ? t.children(b.id) : [];
  $('#nc_parent').innerHTML = colleges.map((c) => `<option value="${c.id}">${esc(c.name)}</option>`).join('');
  $('#nodepick').hidden = true;
  $('#coursenew').hidden = false;
};
$('#backpick').onclick = (e) => { e.preventDefault(); $('#coursenew').hidden = true; $('#nodepick').hidden = false; };
$('#nc_go').onclick = async () => {
  try {
    const n = await api('/nodes', { method: 'POST', body: { parent: Number($('#nc_parent').value), kind: 'course', name: $('#nc_name').value } });
    const d = await api(`/nodes/${n.id}`);
    pick(d.node, d.path);
  } catch (err) { toast(err.message, true); }
};

// ---------------------------------------------------------------- filename recognition

/** Guesses name parts from an original file name (rule-based 自动识别). */
function guess(name) {
  const stem = name.replace(/\.[^.]+$/, '');
  const ext = (/\.([^.]+)$/.exec(name)?.[1] || '').toLowerCase();
  const g = { year: '', term: '', type_word: '', paper: '', with_answer: false, extra: '' };
  let m;
  if ((m = /(20\d{2})\s*[-–~至]\s*(20\d{2})/.exec(stem))) g.year = `${m[1]}-${m[2]}`;
  else if ((m = /(?<!\d)(\d{2})\s*[-–]\s*(\d{2})(?!\d)/.exec(stem)) && Number(m[2]) === Number(m[1]) + 1) g.year = `20${m[1]}-20${m[2]}`;
  else if ((m = /(?<!\d)(20\d{2})(0[1-9]|1[0-2])(?!\d)/.exec(stem))) g.year = `${m[1]}${m[2]}`;
  else if ((m = /(?<!\d)(20\d{2})(?!\d)/.exec(stem))) g.year = m[1];
  if (/第一学期|秋季|秋/.test(stem)) g.term = '秋';
  else if (/第二学期|春季|春/.test(stem)) g.term = '春';
  else if (/暑期|暑假|夏季学期|暑/.test(stem)) g.term = '暑';
  if ((m = /([ABC])\s*卷/i.exec(stem))) g.paper = `${m[1].toUpperCase()}卷`;
  if (/答案|解答|解析|参考答案|评分标准|solution|sln|\bkey\b/i.test(stem)) g.with_answer = true;
  if ((m = /(\d{2,4})\s*题/.exec(stem))) g.extra = `${m[1]}题`;
  const rules = [
    [/期中/, '期中试卷'], [/期末/, '期末试卷'], [/小测|测验|quiz/i, '小测'], [/思考题/, '思考题'], [/题库|刷题|选择题/, '题库'],
    [/真题|试卷|试题|往年|卷|exam|final|midterm/i, '往年试卷'], [/单词/, '单词表'], [/提纲|大纲/, '提纲'], [/重点|复习|考点/, '重点'],
    [/笔记|note/i, '笔记'], [/实验|报告/, '实验报告'], [/讲义/, '讲义'], [/课件|slides?|lecture/i, '课件'], [/教材|课本|教科书|第.版|edition/i, '教材'],
    [/模板|template/i, '模板'],
  ];
  // "期中复习重点" is notes, not a paper: study-material words win unless the name says 试卷/考试.
  const isPaper = /试卷|试题|考试|真题|[ABC]\s*卷/i.test(stem);
  const notes = [[/思考题/, '思考题'], [/题库|刷题/, '题库'], [/单词/, '单词表'], [/提纲|大纲/, '提纲'], [/重点|考点|复习/, '重点'], [/笔记/, '笔记']];
  if (!isPaper) for (const [re, w] of notes) if (re.test(stem)) { g.type_word = w; break; }
  if (!g.type_word) for (const [re, w] of rules) if (re.test(stem)) { g.type_word = w; break; }
  if (!g.type_word) {
    if (['ppt', 'pptx', 'key'].includes(ext)) g.type_word = '课件';
    else if (['zip', 'rar', '7z'].includes(ext)) g.type_word = '合集';
    else if (['html', 'exe', 'py'].includes(ext)) g.type_word = '工具';
    else if (g.with_answer) g.type_word = '往年试卷';
    else g.type_word = '资料';
  }
  if (g.paper && g.type_word === '资料') g.type_word = '往年试卷';
  return g;
}

function timeOf(r) {
  if (!r.year) return '';
  return r.term && /^\d{4}(-\d{4})?$/.test(r.year) ? `${r.year}${r.term}` : r.year;
}

function genName(r) {
  if (!state.node) return '（先选择分类）';
  let s = state.node.label || state.node.name;
  const t = timeOf(r);
  if (t) s += `_${t}`;
  s += `_${r.type_word}`;
  const d = [r.paper, r.with_answer && '含答案', r.extra].filter(Boolean);
  if (d.length) s += `(${d.join('、')})`;
  const ext = (/\.([^.]+)$/.exec(r.file.name)?.[1] || '').toLowerCase();
  return ext ? `${s}.${ext}` : s;
}

// ---------------------------------------------------------------- rows

function addFiles(files) {
  for (const f of files) {
    if (!f.size) { toast(`${f.name} 是空文件，已跳过`, true); continue; }
    if (f.size > M.limits.max_file) { toast(`${f.name} 太大了`, true); continue; }
    if (state.rows.some((r) => r.file.name === f.name && r.file.size === f.size)) continue;
    const g = guess(f.name);
    const row = { file: f, sel: true, parts: null, hashing: 0, status: '', err: false, done: false, note: '', subtitle: '', ...g };
    const saved = draft.files && draft.files[fileKey(f)];
    if (saved) {
      for (const k of FIELDS) if (k in saved) row[k] = saved[k];
      if (Array.isArray(saved.parts)) row.parts = saved.parts;
      if (saved.upload_id) row.upload_id = saved.upload_id;
      row.status = saved.upload_id ? '已恢复上次填写的信息，上传会从断点继续' : '已恢复上次填写的信息';
    }
    state.rows.push(row);
  }
  saveDraft();
  showDraftNotice();
  renderRows();
  hashQueue();
}

function rowHtml(r, i) {
  const types = M.type_words.map((t) => `<option${t.word === r.type_word ? ' selected' : ''}>${t.word}</option>`).join('');
  const hashed = r.parts ? '' : ` · 校验中 ${Math.round(r.hashing * 100)}%`;
  return `<div class="frow${r.done ? ' done' : r.err ? ' err' : ''}" data-i="${i}">
    <div class="head"><input type="checkbox" data-f="sel"${r.sel ? ' checked' : ''}${r.done ? ' disabled' : ''}>
      <span class="orig">${esc(r.file.name)} <span class="faint">· ${fmtSize(r.file.size)}${hashed}</span></span>
      ${r.done ? '' : `<button class="btn sm" data-x="del" type="button">移除</button>`}</div>
    <div class="gen">→ ${esc(genName(r))}</div>
    <div class="opts">
      <label>类型<select class="input" data-f="type_word">${types}</select></label>
      <label>学年 / 年份<select class="input" data-f="year"><option value="">不填</option>${yearOptions(r.year)}</select></label>
      <label>学期<select class="input" data-f="term"><option value="">不填</option>${['秋', '春', '暑'].map((t) => `<option${t === r.term ? ' selected' : ''}>${t}</option>`).join('')}</select></label>
      <label>卷别<select class="input" data-f="paper"><option value="">无</option>${M.papers.map((p) => `<option${p === r.paper ? ' selected' : ''}>${p}</option>`).join('')}</select></label>
      <label>补充（如 201题）<input class="input" data-f="extra" value="${esc(r.extra)}" maxlength="20"></label>
      <label style="flex-direction:row;align-items:center;gap:6px;padding-bottom:8px"><input type="checkbox" data-f="with_answer"${r.with_answer ? ' checked' : ''}> 含答案</label>
      <label class="wide" style="grid-column:1/-1">小标题（选填，公开显示，说明具体内容；留空则在原文件名能看懂时自动使用它）<input class="input" data-f="subtitle" value="${esc(r.subtitle || '')}" maxlength="80" placeholder="${esc(r.file.name.replace(/\.[^.]+$/, ''))}"></label>
      <label class="wide" style="grid-column:1/-1">备注（选填，公开显示，如“只有选择题答案”）<input class="input" data-f="note" value="${esc(r.note)}" maxlength="500"></label>
    </div>
    ${r.status ? `<div class="st ${r.err ? 'bad' : ''}" style="color:${r.err ? 'var(--bad)' : r.done ? 'var(--ok)' : 'var(--muted)'}">${r.status}</div>` : ''}
  </div>`;
}

function renderRows() {
  $('#rows').innerHTML = state.rows.map(rowHtml).join('');
  $('#bulk').hidden = state.rows.length < 2;
  const pending = state.rows.filter((r) => !r.done).length;
  $('#submit').textContent = pending > 1 ? `上传并提交 ${pending} 个文件` : '上传并提交';
}

function updateRow(i) {
  const el = document.querySelector(`.frow[data-i="${i}"]`);
  if (el) el.outerHTML = rowHtml(state.rows[i], i);
}

$('#rows').addEventListener('change', (e) => {
  const row = e.target.closest('.frow');
  const f = e.target.dataset.f;
  if (!row || !f) return;
  const r = state.rows[Number(row.dataset.i)];
  r[f] = e.target.type === 'checkbox' ? e.target.checked : e.target.value;
  saveDraft();
  if (f !== 'note' && f !== 'sel' && f !== 'subtitle') row.querySelector('.gen').textContent = `→ ${genName(r)}`;
});
$('#rows').addEventListener('input', (e) => {
  const row = e.target.closest('.frow');
  const f = e.target.dataset.f;
  if (!row || (f !== 'extra' && f !== 'note' && f !== 'subtitle')) return;
  const r = state.rows[Number(row.dataset.i)];
  r[f] = e.target.value;
  saveDraft();
  if (f === 'extra') row.querySelector('.gen').textContent = `→ ${genName(r)}`;
});
$('#rows').addEventListener('click', (e) => {
  const b = e.target.closest('[data-x="del"]');
  if (!b || state.busy) return;
  const [gone] = state.rows.splice(Number(b.closest('.frow').dataset.i), 1);
  if (draft.files) delete draft.files[fileKey(gone.file)];
  saveDraft();
  renderRows();
});

$('#b_apply').onclick = () => {
  const v = { type_word: $('#b_type').value, year: $('#b_year').value, term: $('#b_term').value, paper: $('#b_paper').value };
  for (const r of state.rows) {
    if (!r.sel || r.done) continue;
    for (const [k, x] of Object.entries(v)) if (x) r[k] = x;
    if ($('#b_ans').checked) r.with_answer = true;
  }
  saveDraft();
  renderRows();
};
$('#b_all').onchange = (e) => { for (const r of state.rows) if (!r.done) r.sel = e.target.checked; renderRows(); };

$('#file').onchange = (e) => { addFiles([...e.target.files]); e.target.value = ''; };
const drop = $('#drop');
drop.ondragover = (e) => { e.preventDefault(); drop.classList.add('over'); };
drop.ondragleave = () => drop.classList.remove('over');
drop.ondrop = (e) => { e.preventDefault(); drop.classList.remove('over'); addFiles([...e.dataTransfer.files]); };

// ---------------------------------------------------------------- hashing (one file at a time)

let hasher = null;
function loadHasher() {
  if (!hasher) {
    hasher = new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = '/vendor/sha256.umd.min.js';
      s.onload = () => resolve(window.hashwasm);
      s.onerror = () => reject(new Error('加载校验组件失败'));
      document.head.appendChild(s);
    });
  }
  return hasher;
}

let hashing = false;
async function hashQueue() {
  if (hashing) return;
  hashing = true;
  try {
    const hw = await loadHasher();
    for (;;) {
      const i = state.rows.findIndex((r) => !r.parts);
      if (i < 0) break;
      const r = state.rows[i];
      const f = r.file;
      const parts = [];
      const CHUNK = 4 * 1024 * 1024;
      let done = 0;
      let lastPaint = 0;
      for (let start = 0; start < f.size; start += M.limits.max_part) {
        const end = Math.min(f.size, start + M.limits.max_part);
        const h = await hw.createSHA256();
        h.init();
        for (let off = start; off < end; off += CHUNK) {
          h.update(new Uint8Array(await f.slice(off, Math.min(end, off + CHUNK)).arrayBuffer()));
          done = Math.min(f.size, off + CHUNK);
          r.hashing = done / f.size;
          if (performance.now() - lastPaint > 300) { lastPaint = performance.now(); updateRow(state.rows.indexOf(r)); }
        }
        parts.push({ start, end, size: end - start, sha256: h.digest('hex') });
      }
      r.parts = parts;
      saveDraft();
      updateRow(state.rows.indexOf(r));
    }
  } catch (e) {
    toast(e.message, true);
  } finally {
    hashing = false;
  }
}

// ---------------------------------------------------------------- upload

function sendPart(target, blob, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open(target.method, target.url);
    for (const [k, v] of target.headers) xhr.setRequestHeader(k, v);
    // Same-origin relay requires the anti-CSRF header; the cross-origin Worker must not get it.
    if (target.url.startsWith('/')) xhr.setRequestHeader('X-XMUHub', '1');
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

async function uploadRow(r, bar, base, total) {
  while (!r.parts) await new Promise((res) => setTimeout(res, 300));
  const f = r.file;
  r.status = '正在准备…';
  updateRow(state.rows.indexOf(r));
  // Resume the upload from before a refresh when the server still has it.
  let plan = null;
  if (r.upload_id) {
    plan = await api(`/uploads/${r.upload_id}`).catch(() => null);
    if (!plan || plan.parts.length !== r.parts.length) plan = null;
  }
  if (!plan) {
    plan = await api('/uploads', { method: 'POST', body: { filename: f.name, mime: f.type, parts: r.parts.map((p) => ({ size: p.size, sha256: p.sha256 })) } });
  }
  r.upload_id = plan.upload_id;
  saveDraft();
  if (!plan.dedup) {
    let sent = 0;
    for (const pp of plan.parts) {
      if (pp.done) { sent += pp.size; continue; }
      const part = r.parts[pp.index];
      for (let attempt = 0; ; attempt++) {
        try {
          r.status = plan.parts.length > 1 ? `上传第 ${pp.index + 1} / ${plan.parts.length} 卷…` : '上传中…';
          updateRow(state.rows.indexOf(r));
          const target = attempt === 0 ? pp.target : (await api(`/uploads/${plan.upload_id}/parts/${pp.index}/renew`, { method: 'POST' })).target;
          const receipt = await sendPart(target, f.slice(part.start, part.end), (n) => {
            bar.style.width = `${Math.round(((base + sent + n) / total) * 100)}%`;
          });
          await api(`/uploads/${plan.upload_id}/parts/${pp.index}`, { method: 'POST', body: { asset_id: receipt.id ?? null } });
          break;
        } catch (err) {
          if (attempt >= 2) throw err;
          r.status = `${esc(err.message)}，重试中…`;
          updateRow(state.rows.indexOf(r));
          await new Promise((res) => setTimeout(res, 1500 * (attempt + 1)));
        }
      }
      sent += pp.size;
    }
  }
  const body = {
    upload_id: plan.upload_id, node: state.node.id, time: timeOf(r), type_word: r.type_word,
    paper: r.paper, with_answer: r.with_answer, extra: r.extra, note: r.note,
    ...(r.subtitle && r.subtitle.trim() ? { subtitle: r.subtitle } : {}),
  };
  let res;
  try {
    res = await api('/resources', { method: 'POST', body });
  } catch (err) {
    // A resumed upload that the server has expired or already used: start it over once.
    if (err.status === 404 || err.status === 409) { r.upload_id = null; saveDraft(); throw new Error(`${err.message}，请再点一次上传重试`); }
    throw err;
  }
  r.done = true;
  r.sel = false;
  r.upload_id = null;
  if (draft.files) delete draft.files[fileKey(r.file)];
  saveDraft();
  r.status = res.status === 'published'
    ? `✓ 已发布：<a href="/r/${res.id}">${esc(res.filename)}</a>`
    : `✓ 已提交，等待审核：<a href="/r/${res.id}">${esc(res.filename)}</a>`;
}

$('#submit').onclick = async () => {
  if (!state.node) return toast('请先选择分类', true);
  const todo = state.rows.filter((r) => !r.done);
  if (!todo.length) return toast('请先添加文件', true);
  state.busy = true;
  $('#submit').disabled = true;
  $('#prog').hidden = false;
  const bar = $('#prog').firstElementChild;
  const total = todo.reduce((s, r) => s + r.file.size, 0);
  let base = 0;
  let ok = 0;
  for (const r of todo) {
    r.err = false;
    try {
      await uploadRow(r, bar, base, total);
      ok++;
    } catch (err) {
      r.err = true;
      r.status = esc(err.message);
    }
    base += r.file.size;
    updateRow(state.rows.indexOf(r));
  }
  bar.style.width = '100%';
  state.busy = false;
  $('#submit').disabled = false;
  renderRows();
  $('#status').innerHTML = `<div class="notice ${ok === todo.length ? 'ok' : 'warn'}">完成 ${ok} / ${todo.length} 个文件${ok < todo.length ? '，失败的可以直接再点一次上传重试' : '，谢谢你的分享！'}</div>`;
};
