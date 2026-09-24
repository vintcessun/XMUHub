// In-page previews. Files are fetched from readable mirrors, with a bounded same-origin
// fallback, then rendered in the browser. Loaded on demand; previews don't count as downloads.

import { api, esc, fetchPart, fmtSize } from './app.js';

const V = '/vendor';
const MAX = 80 * 1024 * 1024;
const TEXT_MAX = 2 * 1024 * 1024;

const IMAGE = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', avif: 'image/avif', apng: 'image/apng', bmp: 'image/bmp', svg: 'image/svg+xml', ico: 'image/x-icon' };
const AUDIO = { mp3: 'audio/mpeg', wav: 'audio/wav', m4a: 'audio/mp4', ogg: 'audio/ogg', flac: 'audio/flac', aac: 'audio/aac' };
const VIDEO = { mp4: 'video/mp4', webm: 'video/webm', m4v: 'video/mp4', mov: 'video/mp4', ogv: 'video/ogg' };
const TEXT = new Set(['txt', 'md', 'markdown', 'json', 'xml', 'html', 'htm', 'css', 'js', 'ts', 'py', 'c', 'h', 'cpp', 'cc', 'hpp', 'java', 'kt', 'go', 'rs', 'rb', 'php', 'cs', 'swift', 'm', 'r', 'sql', 'sh', 'bat', 'ps1', 'tex', 'bib', 'log', 'ini', 'cfg', 'conf', 'yaml', 'yml', 'toml', 'srt', 'asm', 'v', 'vhd', 'mat', 'ipynb']);
const WORD = new Set(['docx', 'docm', 'dotx']);
const SHEET = new Set(['xlsx', 'xlsm', 'xls', 'xlsb', 'ods', 'csv', 'tsv', 'et']);
const SLIDES = new Set(['pptx', 'ppsx', 'pptm']);
const OPEN_DOCUMENT = new Set(['odt', 'ott', 'odp', 'otp']);
const ARCHIVE = new Set(['zip', 'rar', '7z', 'tar', 'gz', 'tgz', 'bz2', 'xz', 'cab', 'iso']);
/** Binary Office 97–2003 formats: text is extracted in the browser; Microsoft's viewer
 * (size limit in MB) is offered for the full layout. */
const LEGACY_OFFICE = { doc: 10, ppt: 10, pps: 10, dot: 10 };
const NOPE = {
  caj: 'CAJ 是知网的专有格式，只能用 CAJViewer 打开，浏览器无法预览。',
  kdh: 'KDH 是知网的专有格式，只能用 CAJViewer 打开，浏览器无法预览。',
  nh: 'NH 是知网的专有格式，只能用 CAJViewer 打开，浏览器无法预览。',
  exe: '这是 Windows 程序，无法在浏览器里预览，请下载后使用（注意安全）。',
  msi: '这是 Windows 安装包，无法在浏览器里预览。',
  apk: '这是安卓安装包，无法在浏览器里预览。',
  dmg: '这是 macOS 安装包，无法在浏览器里预览。',
};

const extOf = (name) => (/\.([^./\\]+)$/.exec(name || '')?.[1] || '').toLowerCase();

const loaded = new Map();
function script(src) {
  if (!loaded.has(src)) {
    loaded.set(src, new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = resolve;
      s.onerror = () => { loaded.delete(src); reject(new Error('加载预览组件失败，请检查网络后重试')); };
      document.head.appendChild(s);
    }));
  }
  return loaded.get(src);
}

const note = (box, msg) => { box.innerHTML = `<div class="notice">${msg}</div>`; };

// ---------------------------------------------------------------- entry point

export async function preview(id, box) {
  const ticket = Symbol('preview');
  box.previewTicket = ticket;
  box.innerHTML = '<p class="small muted">正在加载预览…</p>';
  let plan;
  try { plan = await api(`/resources/${id}/download?peek=1`); } catch (e) { note(box, esc(e.message)); return; }
  if (!box.isConnected || box.previewTicket !== ticket) return;
  const ext = extOf(plan.filename);
  if (NOPE[ext]) return note(box, NOPE[ext]);
  if (plan.size > MAX) return note(box, `文件较大（${fmtSize(plan.size)}），在线预览最多 ${fmtSize(MAX)}，请下载后查看。`);
  if (plan.parts.some((p) => !(p.preview_urls || []).length)) return note(box, '暂时没有支持在线预览的下载线路，请稍后再试或直接下载。');
  box.innerHTML = '<p class="small muted">正在加载预览…</p><div class="progress"><i></i></div>';
  const bar = box.querySelector('.progress i');
  const blobs = [];
  let done = 0;
  try {
    for (const part of plan.parts) {
      blobs.push(await fetchPart(part, part.preview_urls, (n) => { if (bar.isConnected) bar.style.width = `${Math.round(((done + n) / (plan.size || 1)) * 100)}%`; }));
      done += part.size;
      if (!box.isConnected || box.previewTicket !== ticket) return;
    }
  } catch (e) {
    return note(box, `预览加载失败（${esc(e.message)}），请稍后再试或直接下载。`);
  }
  if (!box.isConnected || box.previewTicket !== ticket) return;
  await renderBlob(box, new Blob(blobs), ext, plan.filename, plan);
}

/** Renders file contents by type; also used for files inside archives. */
export async function renderBlob(box, blob, ext, name, plan = null) {
  try {
    if (!ext || !knows(ext)) ext = await sniff(blob) || ext;
    if (ext === 'pdf') return await pdf(box, blob);
    if (IMAGE[ext]) return media(box, blob, IMAGE[ext], (u) => `<img class="preview-img" src="${u}" alt="预览">`);
    if (AUDIO[ext]) return media(box, blob, AUDIO[ext], (u) => `<audio controls src="${u}" style="width:100%"></audio>`);
    if (VIDEO[ext]) return media(box, blob, VIDEO[ext], (u) => `<video controls playsinline src="${u}" class="preview-img"></video>`);
    if (WORD.has(ext)) return await word(box, blob);
    if (SHEET.has(ext)) return await sheet(box, blob, ext);
    if (SLIDES.has(ext)) return await slides(box, blob);
    if (OPEN_DOCUMENT.has(ext)) return await openDocument(box, blob, ext);
    if (ext === 'epub') return await epub(box, blob);
    if (ARCHIVE.has(ext)) return await archive(box, blob, name);
    if (TEXT.has(ext) || ext === 'text') return await text(box, blob, ext);
    if (ext === 'rtf') return await richText(box, blob);
    if (ext === 'doc' || ext === 'dot') return await legacyText(box, blob, 'doc', plan);
    if (ext === 'ppt' || ext === 'pps') return await legacyText(box, blob, 'ppt', plan);
    if (NOPE[ext]) return note(box, NOPE[ext]);
    note(box, `无法识别的文件格式${ext ? `（.${esc(ext)}）` : ''}，请下载后用相应软件打开。大小 ${fmtSize(blob.size)}。`);
  } catch (e) {
    const u = URL.createObjectURL(blob);
    note(box, `预览失败（${esc(e && e.message ? e.message : String(e))}）。可以<a href="${u}" download="${esc(name || 'file')}">保存到本地</a>后打开。`);
  }
}

function knows(ext) {
  return ext === 'pdf' || ext === 'epub' || ext === 'rtf' || IMAGE[ext] || AUDIO[ext] || VIDEO[ext] || TEXT.has(ext) || WORD.has(ext) || SHEET.has(ext) || SLIDES.has(ext) || OPEN_DOCUMENT.has(ext) || ARCHIVE.has(ext) || NOPE[ext] || LEGACY_OFFICE[ext];
}

/** Guesses a type from the first bytes when the extension is missing or unknown. */
async function sniff(blob) {
  const b = new Uint8Array(await blob.slice(0, 4096).arrayBuffer());
  const s = String.fromCharCode(...b.slice(0, 8));
  if (s.startsWith('%PDF')) return 'pdf';
  if (s.startsWith('PK')) {
    await script(`${V}/jszip-3.10.2/jszip.min.js`);
    try {
      const zip = await window.JSZip.loadAsync(blob);
      if (zip.file('word/document.xml')) return 'docx';
      if (zip.file('xl/workbook.xml')) return 'xlsx';
      if (zip.file('ppt/presentation.xml')) return 'pptx';
      const mime = await zip.file('mimetype')?.async('string');
      if (mime === 'application/epub+zip') return 'epub';
      if (mime?.includes('opendocument.text')) return 'odt';
      if (mime?.includes('opendocument.presentation')) return 'odp';
      if (mime?.includes('opendocument.spreadsheet')) return 'ods';
    } catch { /* fall through to archive preview */ }
    return 'zip';
  }
  if (s.startsWith('Rar!')) return 'rar';
  if (b[0] === 0x37 && b[1] === 0x7a && b[2] === 0xbc) return '7z';
  if (b[0] === 0x89 && s.slice(1, 4) === 'PNG') return 'png';
  if (b[0] === 0xff && b[1] === 0xd8) return 'jpg';
  if (s.startsWith('GIF8')) return 'gif';
  if (b[0] === 0x1f && b[1] === 0x8b) return 'gz';
  return b.length && !b.includes(0) ? 'text' : '';
}

function media(box, blob, type, html) {
  const u = URL.createObjectURL(new Blob([blob], { type }));
  box.innerHTML = html(u);
}

// ---------------------------------------------------------------- text

async function text(box, blob, ext) {
  const buf = await blob.slice(0, TEXT_MAX).arrayBuffer();
  let s;
  try { s = new TextDecoder('utf-8', { fatal: true }).decode(buf); } catch { s = new TextDecoder('gb18030').decode(buf); }
  if (ext === 'ipynb') {
    try { s = JSON.parse(s).cells.map((c) => `# ── ${c.cell_type} ──\n${[].concat(c.source).join('')}`).join('\n\n'); } catch { /* show raw */ }
  }
  box.innerHTML = `<pre class="textview"></pre>${blob.size > TEXT_MAX ? `<p class="small faint">只显示了前 ${fmtSize(TEXT_MAX)}，完整内容请下载。</p>` : ''}`;
  box.querySelector('pre').textContent = s;
}

// ---------------------------------------------------------------- pdf

async function pdf(box, blob) {
  await script(`${V}/pdfjs-3.11.174/pdf.min.js`);
  const lib = window.pdfjsLib;
  lib.GlobalWorkerOptions.workerSrc = `${V}/pdfjs-3.11.174/pdf.worker.min.js`;
  const url = URL.createObjectURL(new Blob([blob], { type: 'application/pdf' }));
  const doc = await lib.getDocument({ data: await blob.arrayBuffer(), cMapUrl: `${V}/pdfjs-3.11.174/cmaps/`, cMapPacked: true, isEvalSupported: false }).promise;
  box.innerHTML = `<div class="pdfview"></div><p class="small" style="margin:8px 0 0">共 ${doc.numPages} 页 · <a href="${url}" target="_blank" rel="noopener">在新标签页打开</a></p>`;
  const view = box.querySelector('.pdfview');
  const first = await doc.getPage(1);
  const v1 = first.getViewport({ scale: 1 });
  const pages = [];
  for (let i = 1; i <= doc.numPages; i++) {
    const holder = document.createElement('div');
    holder.className = 'pdfpage';
    holder.style.aspectRatio = `${v1.width} / ${v1.height}`;
    holder.dataset.n = i;
    view.appendChild(holder);
    pages.push(holder);
  }
  const draw = async (holder) => {
    if (holder.dataset.done) return;
    holder.dataset.done = '1';
    const page = await doc.getPage(Number(holder.dataset.n));
    const base = page.getViewport({ scale: 1 });
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const vp = page.getViewport({ scale: ((holder.clientWidth || view.clientWidth || 600) * dpr) / base.width });
    const canvas = document.createElement('canvas');
    canvas.width = Math.floor(vp.width);
    canvas.height = Math.floor(vp.height);
    holder.style.aspectRatio = `${vp.width} / ${vp.height}`;
    holder.appendChild(canvas);
    await page.render({ canvasContext: canvas.getContext('2d'), viewport: vp }).promise;
  };
  // Pages draw as they scroll into view; the first one right away.
  draw(pages[0]).catch(() => {});
  if ('IntersectionObserver' in window) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) if (e.isIntersecting) { io.unobserve(e.target); draw(e.target).catch(() => {}); }
    }, { root: view, rootMargin: '800px 0px' });
    pages.forEach((p) => io.observe(p));
  } else {
    for (const p of pages.slice(0, 10)) await draw(p);
  }
}

// ---------------------------------------------------------------- office

async function word(box, blob) {
  await script(`${V}/jszip-3.10.2/jszip.min.js`);
  await script(`${V}/docx-preview-0.4.1/docx-preview.min.js`);
  box.innerHTML = '<div class="docx-host"></div>';
  const host = box.querySelector('.docx-host');
  await window.docx.renderAsync(blob, host, null, { inWrapper: true, ignoreLastRenderedPageBreak: true, experimental: true, useBase64URL: true });
  fitWidth(host, host.querySelector('.docx-wrapper'));
}

/** Scales fixed-width rendered pages down to fit narrow (phone) screens. */
function fitWidth(host, inner) {
  if (!inner) return;
  const fit = () => {
    inner.style.zoom = '';
    const need = inner.scrollWidth;
    const have = host.clientWidth;
    if (need > have && have > 0) inner.style.zoom = String(Math.max(0.3, have / need));
  };
  fit();
  window.addEventListener('resize', fit);
}

async function sheet(box, blob, ext) {
  await script(`${V}/xlsx-0.18.5/xlsx.full.min.js`);
  const X = window.XLSX;
  const wb = ext === 'csv' || ext === 'tsv'
    ? X.read(await textOf(blob), { type: 'string', sheetRows: 2001, FS: ext === 'tsv' ? '\t' : undefined })
    : X.read(await blob.arrayBuffer(), { type: 'array', sheetRows: 2001 });
  box.innerHTML = `<div class="chips sheet-tabs">${wb.SheetNames.map((n, i) => `<button class="chip${i ? '' : ' on'}" data-i="${i}" type="button">${esc(n)}</button>`).join('')}</div>
    <div class="sheet-host"></div><p class="small faint" style="margin:6px 0 0">每个工作表最多显示前 2000 行。</p>`;
  const host = box.querySelector('.sheet-host');
  const show = (i) => {
    host.innerHTML = X.utils.sheet_to_html(wb.Sheets[wb.SheetNames[i]], { header: '', footer: '' });
    box.querySelectorAll('.sheet-tabs .chip').forEach((c) => c.classList.toggle('on', Number(c.dataset.i) === i));
  };
  box.querySelector('.sheet-tabs').onclick = (e) => { const b = e.target.closest('[data-i]'); if (b) show(Number(b.dataset.i)); };
  if (wb.SheetNames.length < 2) box.querySelector('.sheet-tabs').hidden = true;
  show(0);
}

async function textOf(blob) {
  const buf = await blob.arrayBuffer();
  try { return new TextDecoder('utf-8', { fatal: true }).decode(buf); } catch { return new TextDecoder('gb18030').decode(buf); }
}

/** OpenDocument files are ZIP packages; show their text when a layout renderer is unavailable. */
async function openDocument(box, blob, ext) {
  await script(`${V}/jszip-3.10.2/jszip.min.js`);
  const zip = await window.JSZip.loadAsync(blob);
  const content = zip.file('content.xml');
  if (!content) throw new Error('OpenDocument 文件缺少正文');
  const xml = new DOMParser().parseFromString(await content.async('string'), 'application/xml');
  if (xml.querySelector('parsererror')) throw new Error('OpenDocument 正文无法解析');
  const paragraphs = [...xml.getElementsByTagName('text:p'), ...xml.getElementsByTagName('text:h')]
    .sort((a, b) => a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1)
    .map((el) => el.textContent.trim()).filter(Boolean);
  box.innerHTML = `<p class="small muted" style="margin:0 0 8px">.${esc(ext)} 在线显示文字内容，完整排版请下载查看。</p><pre class="textview"></pre>`;
  box.querySelector('pre').textContent = paragraphs.join('\n\n') || '没有可显示的文字（可能只包含图片或图表）。';
}

/** Extract readable text from common RTF control words without injecting its markup. */
async function richText(box, blob) {
  const source = await textOf(blob.slice(0, TEXT_MAX));
  const tokens = source.match(/\\[a-zA-Z]+-?\d* ?|\\'[0-9a-fA-F]{2}|\\[^a-zA-Z]|[{}]|[^{}\\]+/g) || [];
  const destinations = new Set(['fonttbl', 'colortbl', 'stylesheet', 'info', 'pict', 'object', 'header', 'footer', 'headerl', 'headerr', 'footerl', 'footerr']);
  const stack = [{ skip: false, uc: 1 }];
  let output = '';
  let fallback = 0;
  for (const token of tokens) {
    if (output.length >= TEXT_MAX) break;
    if (token === '{') { stack.push({ ...stack.at(-1) }); continue; }
    if (token === '}') { if (stack.length > 1) stack.pop(); continue; }
    const state = stack.at(-1);
    if (token.startsWith('\\')) {
      const word = /^\\([a-zA-Z]+)(-?\d+)? ?$/.exec(token);
      if (word) {
        const [, command, value] = word;
        if (destinations.has(command)) state.skip = true;
        else if (command === 'uc') state.uc = Number(value) || 0;
        else if (command === 'u' && !state.skip) {
          output += String.fromCharCode((Number(value) + 65536) % 65536);
          fallback = state.uc;
        } else if (!state.skip && (command === 'par' || command === 'line')) output += '\n';
        else if (!state.skip && command === 'tab') output += '\t';
      } else if (token === '\\*') state.skip = true;
      else if (!state.skip && fallback) fallback--;
      else if (!state.skip && /^\\'[0-9a-fA-F]{2}$/.test(token)) output += new TextDecoder('windows-1252').decode(Uint8Array.of(parseInt(token.slice(2), 16)));
      else if (!state.skip && ['\\\\', '\\{', '\\}'].includes(token)) output += token[1];
    } else if (!state.skip) {
      const visible = fallback ? token.slice(fallback) : token;
      fallback = Math.max(0, fallback - token.length);
      output += visible;
    }
  }
  box.innerHTML = `<p class="small muted" style="margin:0 0 8px">RTF 在线显示文字内容，完整排版请下载查看。</p><pre class="textview"></pre>${blob.size > TEXT_MAX ? `<p class="small faint">只显示了前 ${fmtSize(TEXT_MAX)}。</p>` : ''}`;
  box.querySelector('pre').textContent = output.trim() || '没有可显示的文字。';
}

async function slides(box, blob) {
  await script(`${V}/pptx-preview-1.0.7/pptx-preview.umd.js`);
  box.innerHTML = '<div class="pptx-host"></div>';
  const host = box.querySelector('.pptx-host');
  const width = Math.min(host.clientWidth || 800, 960);
  const viewer = window.pptxPreview.init(host, { width, height: Math.round((width * 9) / 16), mode: 'list' });
  await viewer.preview(await blob.arrayBuffer());
}

/** Microsoft's online viewer for the full layout of a legacy Office file. */
function officeViewer(box, plan, ext) {
  if (plan.size > LEGACY_OFFICE[ext] * 1024 * 1024) return note(box, `文件较大（${fmtSize(plan.size)}），微软查看器打不开，请下载后查看。`);
  // Microsoft fetches the file itself, so it gets the plain GitHub URL (the last one).
  const direct = plan.parts[0].urls[plan.parts[0].urls.length - 1];
  const src = `https://view.officeapps.live.com/op/embed.aspx?src=${encodeURIComponent(direct)}`;
  box.innerHTML = `<iframe class="preview-frame" src="${esc(src)}" title="预览"></iframe>
    <p class="small faint" style="margin:8px 0 0">由微软 Office 在线查看器显示，加载需要十几秒；一直空白说明当前网络连不上微软，请直接下载。</p>`;
}

// ---------------------------------------------------------------- Office 97–2003 text

/** Shows the text of a .doc / .ppt (read from the binary format), with the full-layout
 * Microsoft viewer as an option when the file is a whole resource. */
async function legacyText(box, blob, kind, plan) {
  await script(`${V}/xlsx-0.18.5/xlsx.full.min.js`);
  const cfb = window.XLSX.CFB.read(new Uint8Array(await blob.arrayBuffer()), { type: 'array' });
  const s = kind === 'doc' ? docText(cfb) : pptText(cfb);
  if (!s.trim()) throw new Error('没有读到文字（可能是扫描件或加密文件）');
  box.innerHTML = `<p class="small muted" style="margin:0 0 8px">旧版 .${kind} 格式，这里只显示文字内容${plan && plan.parts.length === 1 ? '，<a href="#" data-ms>用微软查看器看完整排版</a>' : ''}。</p><pre class="textview"></pre>`;
  box.querySelector('pre').textContent = s;
  const ms = box.querySelector('[data-ms]');
  if (ms) ms.onclick = (e) => { e.preventDefault(); officeViewer(box, plan, kind); };
}

function stream(cfb, name) {
  const f = window.XLSX.CFB.find(cfb, name);
  if (!f || !f.content) return null;
  const c = f.content;
  return c instanceof Uint8Array ? c : Uint8Array.from(c);
}

/** Word 97–2003: walk the piece table (Clx) and decode each piece. */
function docText(cfb) {
  const wd = stream(cfb, 'WordDocument');
  if (!wd) throw new Error('不是 Word 97–2003 文件');
  const dv = new DataView(wd.buffer, wd.byteOffset, wd.byteLength);
  const flags = dv.getUint16(0x0a, true);
  if (flags & 0x0100) throw new Error('文件已加密');
  const table = stream(cfb, flags & 0x0200 ? '1Table' : '0Table');
  if (!table) throw new Error('缺少 Table 流');
  const fcClx = dv.getUint32(0x01a2, true);
  const lcbClx = dv.getUint32(0x01a6, true);
  const tv = new DataView(table.buffer, table.byteOffset, table.byteLength);
  let p = fcClx;
  const end = fcClx + lcbClx;
  while (p < end && table[p] === 0x01) p += 3 + tv.getUint16(p + 1, true); // skip Prc
  if (table[p] !== 0x02) throw new Error('无法解析文字');
  const lcb = tv.getUint32(p + 1, true);
  const plc = p + 5;
  const n = (lcb - 4) / 12;
  const utf16 = new TextDecoder('utf-16le');
  const ansi = new TextDecoder('windows-1252');
  let out = '';
  for (let i = 0; i < n && out.length < 2_000_000; i++) {
    const cp0 = tv.getUint32(plc + i * 4, true);
    const cp1 = tv.getUint32(plc + (i + 1) * 4, true);
    const fc = tv.getUint32(plc + (n + 1) * 4 + i * 8 + 2, true);
    const len = cp1 - cp0;
    if (fc & 0x40000000) {
      const off = (fc & 0x3fffffff) / 2;
      out += ansi.decode(wd.subarray(off, off + len));
    } else {
      out += utf16.decode(wd.subarray(fc, fc + len * 2));
    }
  }
  return cleanText(out.replace(/\x13[^\x14\x15]*\x14?/g, '').replace(/\x15/g, ''));
}

/** PowerPoint 97–2003: collect text atoms from the record tree, slide by slide. */
function pptText(cfb) {
  const pd = stream(cfb, 'PowerPoint Document');
  if (!pd) throw new Error('不是 PowerPoint 97–2003 文件');
  const dv = new DataView(pd.buffer, pd.byteOffset, pd.byteLength);
  const utf16 = new TextDecoder('utf-16le');
  const ansi = new TextDecoder('windows-1252');
  const parts = [];
  let slide = 0;
  const walk = (start, end, depth) => {
    let p = start;
    while (p + 8 <= end && depth < 16) {
      const verInst = dv.getUint16(p, true);
      const type = dv.getUint16(p + 2, true);
      const len = dv.getUint32(p + 4, true);
      const body = p + 8;
      if (body + len > end) break;
      if (type === 0x03ee) { slide++; parts.push(`\n── 第 ${slide} 页 ──`); } // Slide container
      if ((verInst & 0x0f) === 0x0f) walk(body, body + len, depth + 1);
      else if (type === 0x0fa0) parts.push(utf16.decode(pd.subarray(body, body + len)));
      else if (type === 0x0fa8) parts.push(ansi.decode(pd.subarray(body, body + len)));
      p = body + len;
    }
  };
  walk(0, pd.length, 0);
  return cleanText(parts.join('\n'));
}

function cleanText(s) {
  return s.replace(/\r/g, '\n').replace(/\x07/g, '\t').replace(/\x0b/g, '\n').replace(/[\x00-\x08\x0c\x0e-\x1f]/g, '').replace(/\n{3,}/g, '\n\n').trim();
}

// ---------------------------------------------------------------- e-books

async function epub(box, blob) {
  await script(`${V}/jszip-3.10.2/jszip.min.js`);
  await script(`${V}/epubjs-0.3.93/epub.min.js`);
  box.innerHTML = `<div class="epub-host"></div><div class="row" style="justify-content:center;margin-top:8px">
    <button class="btn sm" data-go="prev" type="button">上一页</button><button class="btn sm" data-go="next" type="button">下一页</button></div>`;
  const book = window.ePub(await blob.arrayBuffer());
  const rendition = book.renderTo(box.querySelector('.epub-host'), { width: '100%', height: '100%', spread: 'none' });
  await rendition.display();
  box.onclick = (e) => { const b = e.target.closest('[data-go]'); if (b) b.dataset.go === 'next' ? rendition.next() : rendition.prev(); };
}

// ---------------------------------------------------------------- archives

let archiveLib = null;
async function libarchive() {
  if (!archiveLib) {
    archiveLib = import(`${V}/libarchive-2.0.2/libarchive.js`).then((m) => {
      m.Archive.init({ workerUrl: `${V}/libarchive-2.0.2/worker-bundle.js` });
      return m.Archive;
    });
  }
  return archiveLib;
}

async function archive(box, blob, name) {
  const Archive = await libarchive();
  const a = await Archive.open(new File([blob], name || 'archive'));
  if (await a.hasEncryptedData().catch(() => false)) return note(box, '这个压缩包有密码，无法预览内容，请下载后解压。');
  const entries = (await a.getFilesArray()).filter((e) => e.file).map((e) => ({ path: `${e.path}${e.file.name}`, size: e.file.size, file: e.file }));
  entries.sort((x, y) => x.path.localeCompare(y.path, 'zh'));
  const total = entries.reduce((n, e) => n + (e.size || 0), 0);
  const shown = entries.slice(0, 500);
  box.innerHTML = `<p class="small muted" style="margin:0 0 8px">压缩包内共 ${entries.length} 个文件，解压后约 ${fmtSize(total)}。点文件名预览。</p>
    <ul class="archive-list">${shown.map((e, i) => `<li><a href="#" data-i="${i}">${esc(e.path)}</a><span class="faint small">${fmtSize(e.size)}</span></li>`).join('')}</ul>
    ${entries.length > shown.length ? `<p class="small faint">只列出了前 ${shown.length} 个。</p>` : ''}<div class="archive-view"></div>`;
  const view = box.querySelector('.archive-view');
  box.querySelector('.archive-list').onclick = async (e) => {
    const link = e.target.closest('[data-i]');
    if (!link) return;
    e.preventDefault();
    const entry = shown[Number(link.dataset.i)];
    box.querySelectorAll('.archive-list a').forEach((x) => x.classList.toggle('on', x === link));
    if (entry.size > MAX) return note(view, `压缩包中的这个文件超过 ${fmtSize(MAX)}，请下载后查看。`);
    view.innerHTML = `<p class="small muted">正在解压 ${esc(entry.path)}…</p>`;
    view.scrollIntoView({ block: 'nearest' });
    try {
      const f = await entry.file.extract();
      view.innerHTML = `<div class="row small" style="margin:10px 0 6px"><b style="overflow-wrap:anywhere">${esc(entry.path)}</b><span class="grow"></span><a href="${URL.createObjectURL(f)}" download="${esc(f.name)}">单独保存这个文件</a></div><div></div>`;
      await renderBlob(view.lastElementChild, f, extOf(entry.path), f.name);
    } catch (err) {
      note(view, `解压失败（${esc(err.message || err)}）`);
    }
  };
}
