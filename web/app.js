'use strict';
const $ = id => document.getElementById(id);
const node = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text !== undefined) n.textContent = text; return n; };
const pretty = value => JSON.stringify(value, null, 2);
const own = (obj, key) => Object.prototype.hasOwnProperty.call(obj || {}, key);
const state = { projects: [], project: null, protocols: [], current: null, selected: new Set(), tab: 'document', resultTab: 'body', response: null, preview: null, drafts: new Map(), expanded: new Set(['response.data']), branch: 0, epoch: 0, edit: null, token: '' };
let toastTimer;
function toast(message, bad = false) { $('toast').textContent = message; $('toast').classList.toggle('error', bad); $('toast').hidden = false; clearTimeout(toastTimer); toastTimer = setTimeout(() => $('toast').hidden = true, bad ? 8500 : 4000); }
async function api(path, options = {}) {
    const headers = { 'X-Proto-CSRF': document.querySelector('meta[name="csrf-token"]').content, ...options.headers };
    if (state.token) headers.Authorization = `Bearer ${state.token}`;
    if (options.body && !(options.body instanceof FormData)) { headers['Content-Type'] = 'application/json'; options.body = JSON.stringify(options.body); }
    const response = await fetch(path, { ...options, headers });
    const text = await response.text(); let data;
    try { data = JSON.parse(text); } catch { data = { error: text || `HTTP ${response.status}` }; }
    if (!response.ok) {
        if (response.status === 401 && !$('auth-dialog').open) $('auth-dialog').showModal();
        const error = new Error(data.error || `HTTP ${response.status}`); error.status = response.status; error.completed = data.completed; throw error;
    }
    return data;
}
const projectApi = (suffix, id = state.project.id) => `/api/projects/${id}/${suffix}`;
function busy(button, text) { const before = button.textContent; button.disabled = true; button.textContent = text; return () => { button.disabled = false; button.textContent = before; }; }
function message(title, description, loading = false) {
    $('workspace').hidden = true; const box = $('page-message'); box.hidden = false; box.replaceChildren();
    if (loading) box.append(node('span', 'loader'));
    box.append(node('h2', '', title), node('p', '', description));
}
function tags(container, values) { container.replaceChildren(...values.map(tag => node('span', 'tag', tag))); }
function operation() { if (!state.current) return {}; return Object.values(state.current.openapi.paths[state.current.path] || {})[0] || {}; }
function resolve(raw, seen = new Set()) {
    if (!raw || typeof raw !== 'object') return {};
    if (raw.$ref) {
        if (seen.has(raw.$ref)) return { ...raw, recursive: true };
        const next = new Set(seen); next.add(raw.$ref);
        let target = state.current.openapi; for (const part of raw.$ref.slice(2).split('/')) target = target?.[part.replace(/~1/g, '/').replace(/~0/g, '~')];
        const local = { ...raw }; delete local.$ref; return { ...resolve(target, next), ...local };
    }
    return raw;
}
function typeName(raw) {
    const s = resolve(raw);
    if (s.anyOf) return s.anyOf.map(typeName).filter((v, i, a) => a.indexOf(v) === i).join(' | ');
    if (s.oneOf) return `oneOf (${s.oneOf.length})`;
    if (Array.isArray(s.type)) return s.type.map(type => typeName({ ...s, type })).join(' | ');
    if (s.type === 'array') return `array<${typeName(s.items)}>`;
    if (s.type === 'object' && s.additionalProperties && !s.properties) return `map<string, ${s.additionalProperties === true ? 'any' : typeName(s.additionalProperties)}>`;
    if (s.format === 'uint64' || s.format === 'int64') return `${s.format} · string`;
    if (s.format === 'int32') return 'int32';
    if (s.type === 'integer' && s.minimum === 0 && s.maximum <= 4294967295) return 'uint32';
    if (s.format === 'binary') return 'file';
    if (s.type === 'object' && raw?.$ref) return raw.$ref.split('/').pop().replace(/\.(request|response)$/, '').split('.').pop();
    return s.type || (s.properties ? 'object' : 'any');
}
function objectSchema(raw) {
    let s = resolve(raw);
    if (s.anyOf) s = resolve(s.anyOf.find(x => x.type !== 'null') || {});
    if (s.type === 'array') s = resolve(s.items);
    if (s.additionalProperties && !s.properties && typeof s.additionalProperties === 'object') s = resolve(s.additionalProperties);
    if (s.allOf) {
        const properties = { ...(s.properties || {}) }, required = [...(s.required || [])];
        for (const part of s.allOf) { const child = objectSchema(part); Object.assign(properties, child.properties || {}); required.push(...(child.required || [])); }
        return { ...s, properties, required: [...new Set(required)] };
    }
    return s;
}
function bodySchema() { const op = operation(); const media = Object.keys(op.requestBody?.content || {})[0]; const s = op.requestBody?.content?.[media]?.schema || {}; return s.oneOf ? s.oneOf[state.branch] || s.oneOf[0] : s; }
function sample(raw, depth = 0, visited = new Set()) {
    if (own(raw, 'default')) return raw.default;
    if (raw?.examples?.length) return raw.examples[0];
    const s = resolve(raw);
    if (own(s, 'default')) return s.default;
    if (s.examples?.length) return s.examples[0];
    if (own(s, 'const')) return s.const;
    if (s.enum?.length) return s.enum.find(v => v !== null) ?? null;
    if (depth > 6 || s.recursive) return {};
    if (raw?.$ref) { if (visited.has(raw.$ref)) return {}; visited = new Set(visited); visited.add(raw.$ref); }
    if (s.oneOf || s.anyOf) return sample((s.oneOf || s.anyOf).find(x => x.type !== 'null') || {}, depth + 1, visited);
    const shape = objectSchema(s);
    if (shape.properties) {
        const result = {};
        for (const [name, field] of Object.entries(shape.properties)) {
            if (shape.required?.includes(name) || own(field, 'default') || field.examples?.length || own(field, 'const')) result[name] = sample(field, depth + 1, visited);
        }
        return result;
    }
    const type = Array.isArray(s.type) ? s.type.find(t => t !== 'null') : s.type;
    if (type === 'array') return [];
    if (type === 'object') return {};
    if (type === 'boolean') return false;
    if (type === 'integer' || type === 'number') return s.minimum ?? 0;
    if (type === 'null') return null;
    return s.format === 'uint64' || s.format === 'int64' ? '0' : '';
}
function schemaTable(container, raw, scope, parameterList) {
    container.replaceChildren(); const shape = objectSchema(raw);
    if (!parameterList && !shape.properties) {
        container.append(node('div', 'schema-note', Object.keys(shape).length ? `内容类型：${typeName(raw)}` : '此接口没有请求参数')); return;
    }
    if (!parameterList && !Object.keys(shape.properties || {}).length) { container.append(node('div', 'schema-note', '无字段定义')); return; }
    if (parameterList && !parameterList.length) return;
    const wrap = node('div', 'schema-table-wrap'), table = node('table', 'schema-table');
    const head = node('thead'), heading = node('tr'); for (const title of ['字段', '类型', '必填', '说明', '默认 / 示例']) heading.append(node('th', '', title)); head.append(heading); table.append(head);
    const body = node('tbody');
    function row(name, field, required, depth, prefix, ancestors, location) {
        const key = `${scope}.${prefix}${name}`, schema = resolve(field), nested = objectSchema(field);
        const ref = field?.$ref || (schema.items || {}).$ref;
        const expandable = nested.properties && !ancestors.has(ref) && depth < 6 && Object.keys(nested.properties).length;
        const tr = node('tr'), nameCell = node('td');
        nameCell.classList.add(`depth-${depth}`);
        if (expandable) { const toggle = node('button', 'tree-toggle', state.expanded.has(key) ? '▾' : '▸'); toggle.title = '展开 / 收起字段'; toggle.onclick = () => { state.expanded.has(key) ? state.expanded.delete(key) : state.expanded.add(key); renderDocument(); }; nameCell.append(toggle); }
        nameCell.append(document.createTextNode(name)); if (location) nameCell.append(node('span', 'field-location', location)); tr.append(nameCell, node('td', '', typeName(field)), node('td', required ? 'required' : 'optional', required ? '是' : '否'));
        const desc = node('td'); desc.append(node('div', 'schema-description', schema.description || '—')); tr.append(desc);
        const example = own(schema, 'default') ? JSON.stringify(schema.default) : schema.examples?.length ? JSON.stringify(schema.examples[0]) : schema.enum ? schema.enum.map(String).join(' / ') : '—';
        const exampleCell = node('td', '', example); exampleCell.title = example; tr.append(exampleCell); body.append(tr);
        if (expandable && state.expanded.has(key)) { const next = new Set(ancestors); if (ref) next.add(ref); for (const [child, value] of Object.entries(nested.properties)) row(child, value, nested.required?.includes(child), depth + 1, `${prefix}${name}.`, next); }
    }
    if (parameterList) for (const p of parameterList) row(p.name, p.schema || p.content?.['application/json']?.schema || {}, p.required, 0, '', new Set(), p.in);
    else for (const [name, field] of Object.entries(shape.properties || {})) row(name, field, shape.required?.includes(name), 0, '', new Set());
    table.append(body); wrap.append(table); container.append(wrap);
}
function renderDocument() {
    const data = state.current, op = operation();
    $('description').textContent = data.description.trim() || '暂无额外说明'; $('maintainers').textContent = data.maintainers.length ? `维护者 · ${data.maintainers.join('、')}` : '';
    const media = Object.keys(op.requestBody?.content || {})[0]; $('request-media').textContent = media || (op.parameters?.length ? 'query / parameters' : '');
    const base = op.requestBody?.content?.[media]?.schema || {}, branch = $('request-branch'); branch.hidden = !base.oneOf;
    branch.replaceChildren(); if (base.oneOf) base.oneOf.forEach((s, i) => { const constValue = Object.values(s.properties || {}).find(x => own(x, 'const'))?.const; const option = node('option', '', constValue || `分支 ${i + 1}`); option.value = i; branch.append(option); }); branch.value = state.branch;
    schemaTable($('request-parameters'), {}, 'parameters', op.parameters || []); $('request-parameters').classList.toggle('has-parameters', Boolean(op.parameters?.length));
    $('request-schema').hidden = !op.requestBody && Boolean(op.parameters?.length); schemaTable($('request-schema'), bodySchema(), 'request');
    const response = op.responses?.['200'] || op.responses?.['302'] || Object.values(op.responses || {})[0] || {};
    const responseMedia = Object.keys(response.content || {})[0]; $('response-media').textContent = responseMedia || (response.headers?.Location ? '302 Redirect' : '');
    schemaTable($('response-schema'), response.content?.[responseMedia]?.schema || { type: 'object', properties: response.headers || {} }, 'response');
    const ex = Object.values(op.requestBody?.content?.[media]?.examples || {})[0]?.value;
    const example = ex !== undefined ? ex : op.requestBody ? sample(bodySchema()) : Object.fromEntries((op.parameters || []).filter(p => p.in === 'query').map(p => [p.name, sample(p.schema || p.content?.['application/json']?.schema || {})]));
    $('document-example').textContent = pretty(example); $('use-example').disabled = false;
}
function renderList() {
    const list = $('protocol-list'), scroll = list.scrollTop, query = $('search').value.trim().toLowerCase(), tag = $('tag-filter').value;
    const visible = state.protocols.filter(p => (!tag || p.tags.includes(tag)) && (!query || `${p.title} ${p.path} ${p.source} ${p.tags.join(' ')} ${p.category_path.join('/')}`.toLowerCase().includes(query)));
    $('protocol-count').textContent = query || tag ? `${visible.length}/${state.protocols.length}` : state.protocols.length;
    const groups = new Map(); for (const p of visible) { const category = p.category_path.join(' / ') || p.source.split('/')[0]; if (!groups.has(category)) groups.set(category, []); groups.get(category).push(p); }
    const fragment = document.createDocumentFragment();
    for (const [category, protocols] of groups) {
        const group = node('details', 'group'); group.open = true; const summary = node('summary'); summary.append(node('span', '', category), node('span', '', protocols.length)); group.append(summary);
        for (const p of protocols) {
            const row = node('div', `proto-row${state.current?.source === p.source ? ' active' : ''}`), check = document.createElement('input'); check.type = 'checkbox'; check.checked = state.selected.has(p.source); check.title = '选择用于批量同步'; check.setAttribute('aria-label', `选择 ${p.title}`);
            check.onchange = () => { check.checked ? state.selected.add(p.source) : state.selected.delete(p.source); renderSelection(); };
            const button = node('button', 'proto-link'); button.title = `${p.title}\n${p.path}`; button.append(node('span', `method ${p.method.toLowerCase()}`, p.method), node('span', 'label', p.title)); button.onclick = () => loadProtocol(p.source);
            row.append(check, button); group.append(row);
        }
        fragment.append(group);
    }
    list.replaceChildren(fragment); if (!visible.length) list.append(node('div', 'list-message', '没有匹配的协议\n试试其他关键词或标签')); list.scrollTop = scroll;
}
function renderSelection() { $('selection-count').textContent = state.selected.size ? `已选择 ${state.selected.size} 个接口` : '未选择批量接口'; $('clear-selection').hidden = !state.selected.size; if (state.current) renderPublish(); }
function rememberDraft() {
    if (!state.project || !state.current) return;
    state.drafts.set(`${state.project.id}:${state.current.source}`, { base: $('base-url').value, path: $('test-path').value, method: $('test-method').value, query: $('query-editor').value, headers: $('headers-editor').value, body: $('body-editor').value, content: $('content-type').value, timeout: $('timeout').value, response: state.response });
}
function fillExample() {
    const op = operation(), media = Object.keys(op.requestBody?.content || {})[0] || 'application/json';
    if (![...$('content-type').options].some(o => o.value === media)) { const option = node('option', '', media); option.value = media; $('content-type').append(option); }
    $('content-type').value = media;
    const full = Object.values(op.requestBody?.content?.[media]?.examples || {})[0]?.value;
    $('body-editor').value = pretty(full !== undefined ? full : sample(bodySchema()));
    $('query-editor').value = pretty(Object.fromEntries((op.parameters || []).filter(p => p.in === 'query').map(p => [p.name, sample(p.schema || p.content?.['application/json']?.schema || {})])));
    $('body-editor').disabled = ['GET', 'HEAD'].includes($('test-method').value); renderFiles(true);
}
function addFile(name = 'file') { const row = node('div', 'file-row'), key = document.createElement('input'), file = document.createElement('input'), remove = node('button', 'icon-button', '×'); key.type = 'text'; key.value = name; key.placeholder = '字段名'; key.setAttribute('aria-label', '文件字段名'); file.type = 'file'; file.setAttribute('aria-label', `选择 ${name} 文件`); remove.title = '移除文件字段'; remove.onclick = () => row.remove(); row.append(key, file, remove); $('file-rows').append(row); }
function renderFiles(reset = false) {
    const multipart = $('content-type').value === 'multipart/form-data'; $('file-uploads').hidden = !multipart;
    if (reset) { $('file-rows').replaceChildren(); if (multipart) { const fields = Object.entries(objectSchema(bodySchema()).properties || {}).filter(([, s]) => resolve(s).format === 'binary'); (fields.length ? fields.map(([name]) => name) : ['file']).forEach(addFile); } }
}
function renderCurrent() {
    const p = state.current; $('page-message').hidden = true; $('workspace').hidden = false;
    $('protocol-title').textContent = p.title; $('protocol-path').textContent = p.path; $('source-label').textContent = p.source; $('source-label').title = p.source;
    $('current-method').textContent = p.method; $('current-method').className = `method ${p.method.toLowerCase()}`; $('current-auth').textContent = p.auth.toUpperCase(); $('deprecated').hidden = !p.deprecated;
    $('breadcrumb-group').textContent = p.category_path.join(' / ') || '接口文档'; tags($('protocol-tags'), p.tags);
    $('source-path').textContent = p.source; $('source-code').textContent = p.source_text;
    state.branch = 0; renderDocument(); renderList();
    const draft = state.drafts.get(`${state.project.id}:${p.source}`);
    let rememberedBase = ''; try { rememberedBase = localStorage.getItem(`protogen:base:${state.project.root}`) || ''; } catch {}
    $('base-url').value = draft?.base || rememberedBase; $('test-path').value = draft?.path || p.path; $('test-method').value = draft?.method || p.method; $('headers-editor').value = draft?.headers || '{}'; $('timeout').value = draft?.timeout || '15';
    fillExample();
    if (draft) { $('query-editor').value = draft.query; $('body-editor').value = draft.body; $('content-type').value = draft.content; renderFiles(true); }
    state.response = draft?.response || null; state.resultTab = 'body'; renderResponse(); state.preview = null; $('publish-result').hidden = true; $('publish-status').textContent = ''; renderPublish(); setTab(state.tab);
}
function setTab(tab) { state.tab = tab; document.querySelectorAll('.tab').forEach(b => b.classList.toggle('active', b.dataset.tab === tab)); document.querySelectorAll('.tab-panel').forEach(p => p.hidden = p.id !== `panel-${tab}`); if (tab === 'request') $('base-url').focus(); }
async function loadProtocol(source, saveDraft = true) {
    if (saveDraft) rememberDraft(); const epoch = ++state.epoch, id = state.project.id;
    $('load-status').textContent = '读取中…';
    try { const data = await api(projectApi(`protocol?source=${encodeURIComponent(source)}`, id)); if (epoch !== state.epoch) return; state.current = data; renderCurrent(); document.querySelector(".proto-row.active")?.scrollIntoView({ block: "center" }); const url = new URL(location.href); url.searchParams.set('project', id); url.searchParams.set('source', source); history.replaceState(null, '', url); }
    catch (e) { if (epoch === state.epoch) { message('无法读取协议', e.message); toast(e.message, true); } }
    finally { if (epoch === state.epoch) { $('load-status').replaceChildren(node('i'), document.createTextNode('本地协议')); } }
}
async function loadProject(id, preferredSource) {
    rememberDraft(); state.project = state.projects.find(p => p.id === Number(id)); state.current = null; state.selected.clear(); state.protocols = []; state.preview = null; $('publish-scope').value = 'current'; $('publish-targets').replaceChildren(); state.epoch++;
    $('project-select').value = id; $('project-name').textContent = state.project.name; $('project-meta').textContent = `${state.project.target === 'typescript' ? 'Node.js / TypeScript' : state.project.target.toUpperCase()} · 本地项目`; $('project-select').title = state.project.root;
    $('search').value = ''; $('tag-filter').value = ''; $('protocol-list').replaceChildren(node('div', 'list-message', '正在加载协议…')); message('读取协议', '正在加载项目中的接口定义…', true); renderSelection();
    const currentId = state.project.id;
    try {
        const data = await api(projectApi('protocols', currentId)); if (currentId !== state.project.id) return; state.protocols = data.protocols;
        const filter = $('tag-filter'); filter.replaceChildren(node('option', '', '全部标签')); filter.firstChild.value = '';
        [...new Set(state.protocols.flatMap(p => p.tags))].sort().forEach(tag => { const option = node('option', '', tag); option.value = tag; filter.append(option); }); renderList();
        if (state.protocols.length) await loadProtocol(state.protocols.some(p => p.source === preferredSource) ? preferredSource : state.protocols[0].source, false); else message('暂无协议', '当前项目没有可查看的接口。');
    } catch (e) { if (currentId === state.project.id) { message('项目加载失败', e.message); $('protocol-list').replaceChildren(node('div', 'list-message', '协议加载失败，请检查源文件后刷新。')); } }
}
async function boot() {
    try {
        const data = await api('/api/projects'); state.projects = data.projects; $('version').textContent = `v${data.version}`;
        $('project-select').replaceChildren(...data.projects.map(p => { const option = node('option', '', p.name); option.value = p.id; return option; }));
        const q = new URL(location.href).searchParams; const id = data.projects.some(p => p.id === Number(q.get('project'))) ? Number(q.get('project')) : data.projects[0].id; await loadProject(id, q.get('source'));
    } catch (e) { message('无法连接工作台', e.message); }
}
function formatResponse(text) { if (/(?:^|[\[,:\s])-?\d{16,}(?=[,\]}\s]|$)/.test(text)) return text; try { return pretty(JSON.parse(text)); } catch { return text; } }
function renderResponse() {
    const r = state.response; $('response-empty').hidden = Boolean(r); $('response-result').hidden = !r; $('response-stats').replaceChildren(); if (!r) return;
    $('response-stats').append(node('span', `status-pill${r.status >= 400 ? ' error' : ''}`, `HTTP ${r.status}`), node('span', '', `${r.elapsed_ms} ms`), node('span', '', r.bytes < 1024 ? `${r.bytes} B` : `${(r.bytes / 1024).toFixed(1)} KiB`));
    $('response-notice').hidden = !r.partial; $('response-notice').textContent = '已达到等待时间或 2 MiB 预览上限，以下为已收到的响应内容。';
    document.querySelectorAll('[data-result]').forEach(b => b.classList.toggle('active', b.dataset.result === state.resultTab));
    $('result-code').textContent = state.resultTab === 'headers' ? pretty(r.headers) : state.resultTab === 'request' ? `${r.method} ${r.url}` : formatResponse(r.body);
    const image = state.resultTab === 'body' && /^image\/(png|jpeg|webp|gif|avif|bmp)(?:;|$)/i.test(r.headers['content-type'] || ''); $('response-image').hidden = !image; if (image) $('response-image').src = `data:${r.headers['content-type'].split(';')[0]};base64,${r.body_base64}`;
}
function jsonObject(editor, label) { const value = JSON.parse($(editor).value || '{}'); if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${label} 必须是 JSON 对象`); return value; }
async function sendRequest() {
    if (!state.current) return; const button = $('send-request'), done = busy(button, '发送中…'), key = `${state.project.id}:${state.current.source}`, id = state.project.id;
    try {
        const headers = jsonObject('headers-editor', '请求头'); if (Object.values(headers).some(v => typeof v !== 'string')) throw new Error('请求头的值必须是字符串');
        const config = { source: state.current.source, base_url: $('base-url').value.trim(), path: $('test-path').value.trim(), method: $('test-method').value, query: jsonObject('query-editor', 'Query 参数'), headers, body: $('body-editor').value, content_type: $('content-type').value, timeout_seconds: Number($('timeout').value) };
        if (!config.base_url) throw new Error('请先填写服务地址');
        try { localStorage.setItem(`protogen:base:${state.project.root}`, config.base_url); } catch {}
        const form = new FormData(); form.append('request', JSON.stringify(config));
        if (config.content_type === 'multipart/form-data') for (const row of $('file-rows').children) { const name = row.querySelector('input[type=text]').value.trim(), file = row.querySelector('input[type=file]').files[0]; if (file) { if (!name) throw new Error('请填写文件字段名'); form.append(name, file, file.name); } }
        rememberDraft(); const response = await api(projectApi('request', id), { method: 'POST', body: form });
        const draft = state.drafts.get(key); if (draft) draft.response = response;
        if (`${state.project.id}:${state.current?.source}` === key) { state.response = response; state.resultTab = 'body'; renderResponse(); $('response-result').scrollIntoView({ behavior: 'smooth', block: 'nearest' }); }
    } catch (e) { toast(e.message, true); } finally { done(); }
}
function download(text, filename, type = 'application/json') { const url = URL.createObjectURL(text instanceof Blob ? text : new Blob([text], { type })); const a = node('a'); a.href = url; a.download = filename; document.body.append(a); a.click(); a.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000); }
function publishSources() { return $('publish-scope').value === 'selected' ? [...state.selected] : state.current ? [state.current.source] : []; }
function renderPublish() {
    if (!state.current) return; const sources = publishSources(); $('publish-source-list').textContent = sources.length ? sources.join('\n') : '在左侧勾选需要批量同步的接口。';
    const previous = new Set([...$('publish-targets').querySelectorAll('input:checked')].map(n => Number(n.value)));
    const env = $('publish-env').value; const matching = sources.map(source => state.protocols.find(p => p.source === source)).filter(Boolean);
    const targets = $('publish-targets'); targets.replaceChildren();
    for (const target of state.current.targets) {
        const allEligible = sources.length && matching.every(p => p.source === state.current.source ? target.eligible : target.configured && (p.export_targets?.length ? p.export_targets.includes(target.key) || p.export_targets.includes(target.name) : target.tags.some(t => p.tags.includes(t))));
        const projectId = env === 'release' ? target.release_id : target.test_id;
        const card = node('label', `target-card${!allEligible || !projectId ? ' disabled' : ''}`), check = document.createElement('input'); check.type = 'checkbox'; check.value = target.id; check.disabled = !allEligible || !projectId;
        check.checked = !check.disabled && (!previous.size || previous.has(target.id));
        const text = node('div'); text.append(node('strong', '', target.name), node('div', 'small muted', projectId ? `项目 ID · ${projectId}` : `未配置${env === 'test' ? '测试' : '正式'}项目 ID`));
        if (!allEligible) text.append(node('div', 'small muted', target.configured ? '协议标签或导出目标不匹配' : '项目未配置访问令牌'));
        const chips = node('div', 'tags'); tags(chips, target.tags); text.append(chips); card.append(check, text); targets.append(card);
    }
    if (!state.current.targets.length) targets.append(node('div', 'schema-note', '当前项目尚未配置 Apifox 发布目标，请先设置 proto/config.json。'));
}
function publishPayload() { const sources = publishSources(), targets = [...$('publish-targets').querySelectorAll('input:checked')].map(n => Number(n.value)); if (!sources.length) throw new Error('请先选择要同步的协议'); if (!targets.length) throw new Error('请先选择可用的发布目标；如未配置测试项目，可切换到正式项目查看'); return { sources, env: $('publish-env').value, targets }; }
async function publishAction(actual) {
    const projectId = state.project.id, source = state.current.source, projectName = state.project.name; const button = $(actual ? 'sync-apifox' : 'preview-apifox'), done = busy(button, actual ? '同步中…' : '生成预览…'); $('publish-status').textContent = actual ? '正在同步所选协议…' : '正在验证发布文档…';
    try {
        const payload = publishPayload(); const result = await api(projectApi(`apifox/${actual ? 'sync' : 'preview'}`, projectId), { method: 'POST', body: payload }); if (state.project.id !== projectId || state.current?.source !== source) { toast(`${projectName} · ${actual ? '同步完成' : '预览完成'}`); return; } state.preview = result;
        $('publish-result').hidden = false; $('publish-result-title').textContent = actual ? '同步结果' : `发布预览 · ${result.jobs.length} 次导入`; $('publish-json').textContent = pretty(result); $('publish-job-list').replaceChildren();
        for (const job of result.jobs || result.completed || []) { const card = node('div', 'publish-job'); card.append(node('strong', '', `${actual ? '✓ ' : ''}${job.project} · ${job.project_id}`), node('div', 'small', job.source)); if (job.paths) card.append(node('div', 'small', Object.entries(job.paths).flatMap(([path, methods]) => Object.keys(methods).map(method => `${method.toUpperCase()} ${path}`)).join('\n'))); $('publish-job-list').append(card); }
        $('publish-status').textContent = actual ? `同步完成 · ${result.completed.length} 次导入` : '预览完成，未发送导入请求'; if (actual) toast('已同步到 Apifox');
    } catch (e) { $('publish-status').textContent = e.message; toast(e.message, true); if (e.completed?.length) { $('publish-result').hidden = false; $('publish-job-list').replaceChildren(); $('publish-json').textContent = pretty({ completed: e.completed, error: e.message }); $('publish-result-title').textContent = '部分同步完成'; } }
    finally { done(); }
}
function openEditor() { if (!state.current) return; state.edit = { project: state.project.id, source: state.current.source, revision: state.current.revision }; $('edit-title').value = state.current.title; $('edit-tags').value = state.current.tags.join('\n'); $('edit-source').textContent = state.current.source; $('edit-error').hidden = true; $('edit-dialog').showModal(); $('edit-title').focus(); }
async function saveMetadata(event) {
    event.preventDefault(); const context = state.edit; if (!context) return; const done = busy($('save-metadata'), '验证并生成中…'); $('edit-error').hidden = true;
    try {
        rememberDraft(); const tags = $('edit-tags').value.split(/\r?\n/).map(t => t.trim()).filter(Boolean);
        const data = await api(projectApi('protocol', context.project), { method: 'PATCH', body: { source: context.source, revision: context.revision, title: $('edit-title').value, tags } });
        if (state.project.id === context.project) { const row = state.protocols.find(p => p.source === context.source); if (row) Object.assign(row, { title: data.title, tags: data.tags }); const filter = $('tag-filter'), chosen = filter.value; filter.replaceChildren(node('option', '', '全部标签')); filter.firstChild.value = ''; [...new Set(state.protocols.flatMap(p => p.tags))].sort().forEach(tag => { const option = node('option', '', tag); option.value = tag; filter.append(option); }); filter.value = [...filter.options].some(o => o.value === chosen) ? chosen : ''; if (state.current?.source === context.source) { state.current = data; renderCurrent(); } else renderList(); }
        $('edit-dialog').close(); toast('标题与标签已保存，项目代码已重新生成');
    } catch (e) { $('edit-error').hidden = false; $('edit-error').textContent = e.message; } finally { done(); }
}
$('project-select').onchange = e => loadProject(e.target.value);
$('search').oninput = renderList; $('tag-filter').onchange = renderList;
$('refresh').onclick = () => loadProject(state.project.id, state.current?.source);
$('generate').onclick = async () => { const done = busy($('generate'), '生成中…'); try { const result = await api(projectApi('generate'), { method: 'POST', body: {} }); toast(`生成完成 · ${result.count} 个接口`); await loadProject(state.project.id, state.current?.source); } catch (e) { toast(e.message, true); } finally { done(); } };
$('clear-selection').onclick = () => { state.selected.clear(); renderSelection(); renderList(); };
document.querySelectorAll('[data-tab]').forEach(b => b.onclick = () => setTab(b.dataset.tab));
$('try-request').onclick = () => setTab('request'); $('use-example').onclick = () => { setTab('request'); fillExample(); };
$('request-branch').onchange = e => { state.branch = Number(e.target.value); renderDocument(); };
$('fill-example').onclick = () => { fillExample(); toast('已填充当前协议示例'); }; $('content-type').onchange = () => renderFiles(true);
$('test-method').onchange = () => $('body-editor').disabled = ['GET', 'HEAD'].includes($('test-method').value);
$('add-file').onclick = () => addFile(); $('send-request').onclick = sendRequest;
document.querySelectorAll('[data-result]').forEach(b => b.onclick = () => { state.resultTab = b.dataset.result; renderResponse(); });
$('download-response').onclick = () => { const r = state.response; if (!r) return; const bytes = Uint8Array.from(atob(r.body_base64), c => c.charCodeAt(0)); download(new Blob([bytes], { type: r.headers['content-type'] || 'application/octet-stream' }), `response-${Date.now()}`); };
$('copy-source').onclick = async () => { try { await navigator.clipboard.writeText(state.current.source_text); toast('源码已复制'); } catch (e) { toast(`复制失败：${e.message}`, true); } };
$('download-openapi').onclick = () => download(pretty(state.current.openapi), `${state.current.source.replace(/\//g, '__').replace('.proto', '')}.openapi.json`);
$('publish-env').onchange = renderPublish; $('publish-scope').onchange = renderPublish;
$('preview-apifox').onclick = () => publishAction(false); $('sync-apifox').onclick = () => publishAction(true);
$('copy-preview').onclick = async () => { try { await navigator.clipboard.writeText($('publish-json').textContent); toast('发布文档已复制'); } catch (e) { toast(e.message, true); } };
$('edit-metadata').onclick = openEditor; $('close-edit').onclick = $('cancel-edit').onclick = () => $('edit-dialog').close(); $('metadata-form').onsubmit = saveMetadata;
$('auth-form').onsubmit = async event => { event.preventDefault(); state.token = $('access-token').value; $('auth-error').hidden = true; try { await api('/api/projects'); $('auth-dialog').close(); $('access-token').value = ''; await boot(); } catch (e) { $('auth-error').textContent = e.message; $('auth-error').hidden = false; } };
document.addEventListener('keydown', e => { if (e.key === '/' && !['INPUT', 'TEXTAREA', 'SELECT'].includes(e.target.tagName) && !$('edit-dialog').open && !$('auth-dialog').open) { e.preventDefault(); $('search').focus(); } if ((e.ctrlKey || e.metaKey) && e.key === 'Enter' && state.tab === 'request' && !$('send-request').disabled) sendRequest(); });
boot();
