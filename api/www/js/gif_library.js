// GIF Library card: drop files or folders (or browse), auto-upload, auto-reindex (parity with the ESP32 firmware)
export function initGifLibrary() {
  const $ = (id) => document.getElementById(id);
  const folderSel = $('gif-lib-folder'), newFolder = $('gif-lib-newfolder'), filesIn = $('gif-lib-files'), dirIn = $('gif-lib-dir');
  const dropZone = $('gif-lib-drop-zone'), pickDir = $('gif-lib-pick-dir'), uploadBtn = $('gif-lib-upload-btn'), progress = $('gif-lib-progress'), table = $('gif-lib-table');
  if (!folderSel || !dropZone) return;
  const toast = (m, t) => (window.showToast ? window.showToast(m, t) : console.log(t, m));
  const headers = () => { const h = {}; const tok = localStorage.getItem('api_token'); if (tok) h['X-API-Token'] = tok; return h; };
  const BATCH = 6;             // files per request: keeps each multipart body small for the ESP32
  const ALLOWED = /\.(gif|png|raw)$/i;
  let busy = false;
  let pending = null;   // Map<folder, File[]> queued by drop/browse, sent when Upload is clicked

  async function loadLibrary() {
    try {
      const res = await fetch('/api/gifs/library', { headers: headers() });
      const data = await res.json();
      const folders = data.folders || {};
      const names = Object.keys(folders).sort((a, b) => a.localeCompare(b));
      const current = folderSel.value;
      folderSel.innerHTML = '';
      names.forEach(n => { const o = document.createElement('option'); o.value = n; o.textContent = `${n} (${folders[n].count})`; folderSel.appendChild(o); });
      if (names.includes(current)) folderSel.value = current;
      table.innerHTML = names.length ? names.map(n => `<a href="#" class="gif-lib-chip" data-folder="${n}" style="color: inherit; text-decoration: none;" title="Show files"><strong>${n}</strong> · ${folders[n].count}</a>`).join('') : '<span>No playlist folders yet.</span>';
      table.querySelectorAll('.gif-lib-chip').forEach(a => a.onclick = (e) => { e.preventDefault(); showFolder(a.dataset.folder); });
    } catch (e) { table.textContent = 'Library unavailable: ' + (e.message || e); }
  }

  function targetFolder() {
    const nf = (newFolder.value || '').trim();
    return nf ? nf : folderSel.value;
  }

  async function uploadBatch(folder, files) {
    const fd = new FormData();
    files.forEach(f => fd.append('file', f, f.name));
    const res = await fetch('/api/gifs/upload?folder=' + encodeURIComponent(folder), { method: 'POST', headers: headers(), body: fd });
    let body = {};
    try { body = await res.json(); } catch (_) {}
    if (!res.ok && !(body.saved && body.saved.length)) throw new Error(body.message || ('HTTP ' + res.status));
    return body;
  }

  // groups: Map<folderName, File[]>
  async function uploadGroups(groups) {
    if (busy) { toast('An upload is already running.', 'info'); return; }
    let total = 0; groups.forEach(v => total += v.length);
    let done = 0, saved = 0, skipped = [];
    busy = true; uploadBtn.disabled = true; dropZone.style.opacity = '0.6';
    try {
      for (const [folder, files] of groups) {
        for (let i = 0; i < files.length; i += BATCH) {
          const batch = files.slice(i, i + BATCH);
          progress.textContent = `Uploading to ${folder}: ${done}/${total}…`;
          const r = await uploadBatch(folder, batch);
          saved += (r.saved || []).length; skipped = skipped.concat(r.skipped || []);
          done += batch.length;
        }
      }
      progress.textContent = `Done: ${saved} saved` + (skipped.length ? `, ${skipped.length} skipped (${skipped.map(s => s.name).slice(0, 5).join(', ')}${skipped.length > 5 ? '…' : ''})` : '') + '.';
      toast(`Uploaded ${saved} file(s)` + (skipped.length ? `, ${skipped.length} skipped` : ''), skipped.length && !saved ? 'error' : 'success');
    } catch (e) {
      progress.textContent = 'Upload failed: ' + (e.message || e);
      toast('Upload failed: ' + (e.message || e), 'error');
    } finally {
      // reindex automatically so the new files are live without a second click (needed on the ESP32, harmless on the Pi)
      try { await fetch('/api/gifs/reindex', { method: 'POST', headers: headers() }); } catch (_) {}
      busy = false; dropZone.style.opacity = '';   // Upload stays disabled until something new is queued
      filesIn.value = ''; dirIn.value = ''; newFolder.value = '';
      loadLibrary();
    }
  }

  // Build folder groups from a list of {file, relPath} and QUEUE them (the Upload button sends them).
  // Top-level folder in relPath = playlist; loose files go to the target folder chosen above.
  function queueSelection(items) {
    const groups = new Map();
    let loose = 0;
    items.forEach(({ file, relPath }) => {
      if (!ALLOWED.test(file.name)) return;
      const parts = (relPath || file.name).split('/').filter(Boolean);
      let folder;
      if (parts.length > 1) folder = parts[0];
      else { folder = targetFolder(); loose++; }
      if (!folder) return;
      if (!groups.has(folder)) groups.set(folder, []);
      groups.get(folder).push(file);
    });
    if (!groups.size) {
      pending = null; uploadBtn.disabled = true;
      progress.textContent = loose === 0 && items.length ? 'Pick a target playlist or type a new name for loose files, then choose them again.' : 'No .gif/.png/.raw files in that selection.';
      return;
    }
    pending = groups; uploadBtn.disabled = false;
    let n = 0; groups.forEach(v => n += v.length);
    progress.textContent = `${n} file(s) queued for ${groups.size} playlist(s): ${Array.from(groups.keys()).join(', ')}. Click Upload to send.`;
  }

  // Walk dropped DataTransfer items (files AND folders) into {file, relPath}
  async function collectDropped(dt) {
    const out = [];
    const items = dt.items ? Array.from(dt.items) : [];
    const entries = items.map(it => (it.webkitGetAsEntry ? it.webkitGetAsEntry() : null)).filter(Boolean);
    if (!entries.length) { Array.from(dt.files || []).forEach(f => out.push({ file: f, relPath: f.name })); return out; }
    const readAll = (dirReader) => new Promise((resolve, reject) => {
      const all = [];
      const step = () => dirReader.readEntries(batch => { if (!batch.length) resolve(all); else { all.push(...batch); step(); } }, reject);
      step();
    });
    const walk = async (entry, prefix) => {
      if (entry.isFile) {
        const file = await new Promise((res, rej) => entry.file(res, rej));
        out.push({ file, relPath: prefix + entry.name });
      } else if (entry.isDirectory) {
        const children = await readAll(entry.createReader());
        for (const c of children) await walk(c, prefix + entry.name + '/');
      }
    };
    for (const e of entries) await walk(e, '');
    return out;
  }

  ['dragenter', 'dragover'].forEach(ev => dropZone.addEventListener(ev, e => { e.preventDefault(); dropZone.style.borderColor = 'var(--accent, #6cf)'; }));
  ['dragleave', 'drop'].forEach(ev => dropZone.addEventListener(ev, e => { e.preventDefault(); dropZone.style.borderColor = 'rgba(255,255,255,0.2)'; }));
  dropZone.addEventListener('drop', async e => { const items = await collectDropped(e.dataTransfer); queueSelection(items); });
  dropZone.addEventListener('click', e => { if (e.target === pickDir) return; filesIn.click(); });
  pickDir.addEventListener('click', e => { e.preventDefault(); e.stopPropagation(); dirIn.click(); });
  filesIn.onchange = () => queueSelection(Array.from(filesIn.files || []).map(f => ({ file: f, relPath: f.name })));
  dirIn.onchange = () => queueSelection(Array.from(dirIn.files || []).map(f => ({ file: f, relPath: f.webkitRelativePath || f.name })));


  // ---- folder contents panel: list files, delete a file, delete the whole folder ----
  const panel = $('gif-lib-files-panel'), panelTitle = $('gif-lib-files-title'), panelFiles = $('gif-lib-file-list'), panelClose = $('gif-lib-files-close'), folderDel = $('gif-lib-folder-delete');
  let openFolder = null;
  const fmtBytes = (b) => b >= 1048576 ? (b / 1048576).toFixed(1) + ' MB' : b >= 1024 ? Math.round(b / 1024) + ' KB' : b + ' B';
  async function showFolder(name) {
    try {
      const res = await fetch('/api/gifs/files?folder=' + encodeURIComponent(name), { headers: headers() });
      if (!res.ok) throw new Error('HTTP ' + res.status);
      const data = await res.json();
      openFolder = name; panel.style.display = '';
      panelTitle.textContent = `${name} · ${data.files.length} file(s)`;
      panelFiles.innerHTML = data.files.length ? data.files.map(f => `<span>${f.name} <span style="opacity:0.6">${fmtBytes(f.bytes)}</span> <a href="#" class="gif-lib-del" data-name="${f.name}" title="Delete this file" style="color: #f87171; text-decoration: none; margin-left: 0.2rem;">✕</a></span>`).join('') : '<span>Empty folder.</span>';
      panelFiles.querySelectorAll('.gif-lib-del').forEach(a => a.onclick = async (e) => {
        e.preventDefault(); const fn = a.dataset.name;
        if (!confirm(`Delete ${fn} from ${name}?`)) return;
        try {
          const r = await fetch(`/api/gifs/file?folder=${encodeURIComponent(name)}&name=${encodeURIComponent(fn)}`, { method: 'DELETE', headers: headers() });
          if (!r.ok) throw new Error('HTTP ' + r.status);
          toast(`Deleted ${fn}`, 'success'); showFolder(name); loadLibrary();
        } catch (err) { toast('Delete failed: ' + (err.message || err), 'error'); }
      });
    } catch (e) { toast('Could not list folder: ' + (e.message || e), 'error'); }
  }
  panelClose.onclick = () => { panel.style.display = 'none'; openFolder = null; };
  folderDel.onclick = async () => {
    if (!openFolder) return;
    if (!confirm(`Delete the whole playlist folder "${openFolder}" and every file in it? This cannot be undone.`)) return;
    try {
      const r = await fetch('/api/gifs/folder?folder=' + encodeURIComponent(openFolder), { method: 'DELETE', headers: headers() });
      if (!r.ok) throw new Error('HTTP ' + r.status);
      const d = await r.json(); toast(`Deleted folder ${openFolder} (${d.files} files)`, 'success');
      panel.style.display = 'none'; openFolder = null; loadLibrary();
    } catch (err) { toast('Delete failed: ' + (err.message || err), 'error'); }
  };

  uploadBtn.onclick = () => { if (!pending) { toast('Choose files or a folder first.', 'error'); return; } const g = pending; pending = null; uploadBtn.disabled = true; uploadGroups(g); };


  loadLibrary();
}
