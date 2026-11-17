'use strict';

window.DevCoordinatorProjectExplorer = (() => {
  const snapshots = new Map();
  const editableStates = ['planned', 'active', 'paused', 'blocked', 'ready', 'abandoned'];
  const finalJobs = new Set(['succeeded', 'completed', 'complete', 'failed', 'cancelled', 'partial', 'blocked']);
  const retiredStates = new Set(['merged', 'retired', 'abandoned']);
  const escape = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const i18n = () => window.DevCoordinatorI18n;
  const t = (key, params) => i18n().t(`projects.${key}`, params);
  const h = (key, params) => i18n().markup(`projects.${key}`, params);
  const icon = name => `<span class="pe-icon pe-icon-${name}" aria-hidden="true"></span>`;
  const short = sha => sha ? String(sha).slice(0, 8) : t('unknown');
  const href = (id, view = 'copies', branch = '', checkout = '') => `#/projects${id ? `/${encodeURIComponent(id)}` : ''}${id ? `?${new URLSearchParams({ view, ...(branch ? { branch } : {}), ...(checkout ? { checkout } : {}) })}` : ''}`;
  const size = value => {
    if (value == null || !Number.isFinite(Number(value))) return t('notMeasured');
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; let n = Number(value), k = 0;
    while (n >= 1024 && k < units.length - 1) { n /= 1024; k++; }
    return `${i18n().number(n, { maximumFractionDigits: k ? 1 : 0 })} ${units[k]}`;
  };
  const date = value => value && Number.isFinite(Date.parse(value)) ? i18n().date(value, { dateStyle: 'medium', timeStyle: 'short' }) : t('notObserved');
  const external = url => { try { const parsed = new URL(url); return parsed.protocol === 'https:' && !parsed.username && !parsed.password ? parsed.href : null; } catch { return null; } };
  const stateLabel = value => t(`state_${value || 'unknown'}`);
  const status = value => `<span class="pe-state pe-state-${/^[a-z_]+$/.test(value || '') ? value : 'unknown'}"><span aria-hidden="true"></span>${escape(stateLabel(value))}</span>`;
  const reasonText = reason => t(`reason_${reason}`) === `projects.reason_${reason}` ? String(reason).replaceAll('_', ' ') : t(`reason_${reason}`);

  async function mount(root, { api, signal, projectId, identity }) {
    await i18n().ensure('projects');
    if (signal.aborted) return;
    const query = new URLSearchParams(location.hash.split('?')[1] || '');
    const s = {
      projectId, view: ['overview', 'copies', 'branches', 'storage', 'history'].includes(query.get('view')) ? query.get('view') : 'copies',
      data: null, projects: [], projectCursor: null, branchId: query.get('branch') || '', checkoutId: query.get('checkout') || '',
      open: new Set(), collapsed: new Set(), commits: new Map(), showCommits: false, query: '', edit: null,
      error: '', refreshing: false, job: null, retireKeys: new Map(), pageRequest: 0, read: 0, queryRead: 0, disposed: false, observer: null,
    };
    const alive = () => !signal.aborted && !s.disposed && root.isConnected;
    const cacheKey = `${identity?.email || 'local'}:${identity?.administrator ? 'admin' : 'member'}:${projectId}`;
    const pendingRemovalKey = `dc2-project-removal:${cacheKey}`;
    const selectedBranch = () => s.data?.branches.find(b => b.branch_id === s.branchId);
    const selectedCopy = () => s.data?.checkouts.find(c => c.worktree_id === s.checkoutId);
    const repository = id => s.data?.repositories.find(r => r.repository_id === id);
    const permissions = id => repository(id)?.permissions || s.data?.permissions || {};
    const route = () => history.replaceState(history.state, '', href(s.projectId, s.view, s.branchId, s.checkoutId));
    const fault = error => { if (!alive() || error?.code === 'stale' || error?.name === 'AbortError') return; s.error = error?.message || t('requestFailed'); paintState(); };
    const key = purpose => `explorer-${purpose}-${crypto.randomUUID()}`;
    const action = async fn => { try { return await fn(); } catch (error) { fault(error); return null; } };

    root.innerHTML = `<section class="pe-shell" data-project-explorer data-ui-region="project-explorer">
      <aside class="pe-projects" aria-label="${escape(t('projectList'))}"><header><h2>${icon('folder')}${h('projects')}</h2><button class="pe-icon-button pe-rail-close" type="button" data-pe-close-rail aria-label="${escape(t('closeProjects'))}">${icon('x')}</button></header><nav data-pe-project-list></nav><button class="btn btn-small" data-pe-more-projects hidden>${h('moreProjects')}</button></aside>
      <div class="pe-content"><header class="pe-heading"><button class="pe-icon-button pe-rail-open" type="button" data-pe-open-rail aria-label="${escape(t('projectList'))}">${icon('menu-2')}</button><h1 data-pe-title tabindex="-1">${h('projects')}</h1><div data-pe-context-links class="pe-context-links"></div></header>
        <nav class="pe-tabs" data-pe-tabs aria-label="${escape(t('projectViews'))}"></nav>
        <div class="pe-toolbar" data-pe-toolbar hidden><form data-pe-search class="pe-search">${icon('search')}<input type="search" name="query" placeholder="${escape(t('searchPlaceholder'))}" aria-label="${escape(t('searchPlaceholder'))}" autocomplete="off"><button class="pe-icon-button" type="submit" aria-label="${escape(t('search'))}">${icon('arrow-right')}</button></form><button class="btn" data-pe-refresh type="button">${icon('refresh')}${h('refresh')}</button><span data-pe-freshness class="pe-freshness" role="status"></span><label class="pe-commits-toggle"><input type="checkbox" data-pe-show-commits><span>${h('showCommits')}</span></label></div>
        <div data-pe-error class="pe-error" role="alert" hidden></div><div data-pe-job class="pe-job" role="status" hidden></div><div data-pe-body class="pe-body"><p class="pe-empty">${h('loadingProjects')}</p></div>
        <section data-pe-inspector class="pe-inspector" aria-label="${escape(t('selectionDetails'))}" hidden></section>
      </div></section>`;
    const find = name => root.querySelector(`[data-pe-${name}]`);
    const shell = root.querySelector('.pe-shell');
    function closeRail() { shell.classList.remove('pe-rail-visible'); find('open-rail').setAttribute('aria-expanded', 'false'); }
    find('open-rail').addEventListener('click', () => { shell.classList.add('pe-rail-visible'); find('open-rail').setAttribute('aria-expanded', 'true'); find('project-list').querySelector('a')?.focus(); });
    find('close-rail').addEventListener('click', () => { closeRail(); find('open-rail').focus(); });
    root.addEventListener('keydown', event => { if (event.key === 'Escape') closeRail(); });
    find('project-list').addEventListener('click', closeRail);
    find('refresh').addEventListener('click', () => action(refresh));
    find('more-projects').addEventListener('click', () => action(() => loadProjects(true)));
    find('search').addEventListener('submit', event => { event.preventDefault(); void action(() => search(new FormData(event.currentTarget).get('query').trim())); });
    find('search').querySelector('input').addEventListener('search', event => { if (!event.target.value) void action(() => search('')); });
    find('show-commits').addEventListener('change', event => { s.showCommits = event.target.checked; if (s.showCommits && s.branchId) s.open.add(s.branchId); paintBody(); if (s.showCommits && selectedBranch()) void loadCommits(selectedBranch()); });
    root.addEventListener('keydown', treeKeyboard);
    root.addEventListener('click', onClick);
    signal.addEventListener('abort', () => { s.disposed = true; s.observer?.disconnect(); root.removeEventListener('click', onClick); root.removeEventListener('keydown', treeKeyboard); root.querySelector('dialog')?.remove(); }, { once: true });
    function paintProjects() {
      find('project-list').innerHTML = s.projects.map(p => `<a class="pe-project" href="${href(p.project_id)}"${p.project_id === s.projectId ? ' aria-current="page"' : ''}>${icon('folder')}<span>${escape(p.name)}</span></a>`).join('') || `<p class="pe-muted">${h('noProjects')}</p>`;
      find('more-projects').hidden = !s.projectCursor;
    }
    function paintState() {
      const box = find('error'); box.hidden = !s.error;
      box.replaceChildren(); if (s.error) { const text = document.createElement('span'); text.textContent = s.error; const retry = document.createElement('button'); retry.type = 'button'; retry.className = 'btn btn-small'; retry.textContent = t('retry'); retry.addEventListener('click', () => action(() => s.projectId ? loadStatus() : loadProjects())); box.append(text, retry); }
      find('refresh').disabled = s.refreshing;
      find('refresh').setAttribute('aria-busy', String(s.refreshing));
      if (s.data) {
        const problems = s.data.repositories.filter(r => r.error || r.refresh_state === 'failed');
        find('freshness').textContent = s.refreshing ? t('refreshing') : problems.length ? t('refreshIncomplete') : s.data.refreshed_at ? `${t('updated')} ${date(s.data.refreshed_at)}` : t('notRefreshed');
        find('freshness').title = problems.map(r => `${r.name}: ${reasonText(r.error || 'unavailable')}`).join('\n');
      }
    }
    function paint() {
      if (!alive()) return;
      const data = s.data;
      find('toolbar').hidden = !data;
      find('title').textContent = data?.project.name || t('projects');
      find('tabs').innerHTML = data ? ['overview', 'copies', 'branches', 'storage', 'history'].map(view => `<a href="${href(s.projectId, view, s.branchId, s.checkoutId)}" data-pe-view="${view}"${s.view === view ? ' aria-current="page"' : ''}>${h(view)}</a>`).join('') : '';
      const repo = data?.repositories.find(r => r.repository_id === selectedBranch()?.repository_id) || data?.repositories[0];
      find('context-links').innerHTML = repo ? [['plan', 'plan'], ['tests', 'tests'], ['deployments', 'deployments']].map(([view, label]) => `<a href="${view === 'plan' ? `#/plan/${encodeURIComponent(repo.repository_id)}` : `#/${view}?repository=${encodeURIComponent(repo.repository_id)}`}">${h(label)}</a>`).join('') : '';
      paintState(); paintBody(); paintInspector();
    }
    function paintBody() {
      if (!alive()) return;
      const body = find('body');
      const focused = document.activeElement?.dataset?.node;
      if (!s.data) {
        body.innerHTML = `<div class="pe-project-index"><h2>${h('chooseProject')}</h2>${s.projects.map(p => `<a href="${href(p.project_id)}">${icon('folder')}<strong>${escape(p.name)}</strong><span>${h('repositoryCount', { count: p.repository_count })}</span><span>${escape(size(p.local_bytes))}</span></a>`).join('') || `<p>${h('noProjects')}</p>`}</div>`;
        return;
      }
      if (s.view === 'history') { paintHistory(body); return; }
      if (s.view === 'storage') { paintStorage(body); return; }
      if (s.view === 'overview') {
        body.innerHTML = `<section class="pe-overview"><h2>${h('projectSources')}</h2>${s.data.repositories.map(r => `<article><div><h3>${escape(r.name)}</h3>${external(r.github_url) ? `<a href="${escape(external(r.github_url))}" target="_blank" rel="noopener noreferrer">${escape(r.github_url)}${icon('external-link')}</a>` : `<p class="pe-muted">${h('githubNotConfigured')}</p>`}</div><span>${escape(r.default_branch || t('notRecorded'))}</span><span>${escape(r.error ? reasonText(r.error) : t('state_' + r.refresh_state))}</span></article>`).join('')}<h2>${h('projectRoots')}</h2>${s.data.roots.map(r => `<p class="pe-root-description">${icon('folder')}<strong>${escape(r.label)}</strong><span>${escape(r.absolute_path || r.relative_path)}</span></p>`).join('')}<div class="pe-overview-footer"><span>${h('measuredLocalSpace')} <strong>${escape(size(s.data.storage.allocated_bytes))}</strong></span><span>${h('unmeasuredCount', { count: s.data.storage.unmeasured_count })}</span></div></section>`;
        return;
      }
      body.innerHTML = `<div class="pe-trees${s.view === 'branches' ? ' pe-branches-view' : ''}" data-pe-trees>
        <section class="pe-tree-panel pe-remote" aria-label="${escape(t('githubTree'))}"><header>${icon('brand-github')}<div><h2>GitHub</h2><p>${h('remoteDescription')}</p></div></header><ul class="pe-tree" role="tree" aria-label="${escape(t('githubTree'))}">${remoteTree()}</ul>${s.data.next_branch_cursor ? `<button class="pe-more btn btn-small" data-pe-more="branches">${h('moreBranches')}</button>` : ''}</section>
        <section class="pe-tree-panel pe-local" aria-label="${escape(t('localTree'))}"><header>${icon('database')}<div><h2>${h('onThisServer')}</h2><p>${h('localDescription')}</p></div></header><ul class="pe-tree" role="tree" aria-label="${escape(t('localTree'))}">${localTree()}</ul>${s.data.next_checkout_cursor ? `<button class="pe-more btn btn-small" data-pe-more="checkouts">${h('moreCopies')}</button>` : ''}</section>
        <svg class="pe-links" aria-hidden="true" data-pe-links></svg></div>`;
      if (focused) body.querySelector(`[data-node="${CSS.escape(focused)}"]`)?.focus({ preventScroll: true });
      connect();
    }
    function disclosure(key, label, children, { level = 1, mark = 'folder', trailing = '' } = {}) {
      const expanded = !s.collapsed.has(key);
      return `<li role="none"><button type="button" role="treeitem" aria-level="${level}" aria-expanded="${expanded}" class="pe-node pe-group" data-node="${escape(key)}" data-pe-disclose="${escape(key)}" tabindex="${level === 1 ? 0 : -1}">${icon(expanded ? 'chevron-down' : 'chevron-right')}${icon(mark)}<strong>${escape(label)}</strong>${trailing}</button>${expanded ? `<ul role="group">${children}</ul>` : ''}</li>`;
    }
    function remoteTree() {
      if (!s.data.repositories.length) return `<li role="none" class="pe-empty">${h('noSources')}</li>`;
      return s.data.repositories.map(repo => {
        const branches = s.data.branches.filter(b => b.repository_id === repo.repository_id);
        const active = branches.filter(b => !retiredStates.has(b.lifecycle));
        const historical = branches.filter(b => retiredStates.has(b.lifecycle));
        const rows = active.map(b => branchRow(b, 2)).join('') + (historical.length ? disclosure('history-' + repo.repository_id, t('completedHistory'), historical.map(b => branchRow(b, 3)).join(''), { level: 2, mark: 'history' }) : '');
        const warning = repo.error ? `<li role="none" class="pe-source-warning">${escape(reasonText(repo.error))}</li>` : '';
        return disclosure('repo-' + repo.repository_id, repo.name, warning + (rows || `<li role="none" class="pe-empty">${h('noBranches')}</li>`), { mark: 'file-text', trailing: repo.default_branch ? `<span class="pe-node-extra">${escape(repo.default_branch)}</span>` : '' });
      }).join('');
    }
    function branchRow(branch, level) {
      const selected = branch.branch_id === s.branchId;
      const commits = s.commits.get(branch.branch_id);
      const expanded = s.showCommits && s.open.has(branch.branch_id);
      const availability = branch.remote_present === false ? (branch.local_present ? t('localOnly') : t('remoteRemoved')) : branch.remote_present == null ? t('remoteUnverified') : '';
      const commitRows = commits?.items?.map(c => `<li role="none" class="pe-commit"><span class="pe-commit-point" aria-hidden="true"></span><a href="${escape(commitUrl(branch, c.sha) || '#')}"${commitUrl(branch, c.sha) ? ' target="_blank" rel="noopener noreferrer"' : ' aria-disabled="true" tabindex="-1"'}><code>${escape(short(c.sha))}</code><span>${escape(c.subject)}</span></a>${c.parents.length ? `<span class="pe-commit-parents" title="${escape(c.parents.join(', '))}">${h('parents')}: ${c.parents.map(p => escape(short(p))).join(', ')}</span>` : ''}</li>`).join('');
      return `<li role="none"><button class="pe-node pe-branch${selected ? ' selected' : ''}" type="button" role="treeitem" aria-level="${level}" aria-selected="${selected}"${s.showCommits ? ` aria-expanded="${expanded}"` : ''} data-node="branch-${escape(branch.branch_id)}" data-pe-branch="${escape(branch.branch_id)}" tabindex="${selected ? 0 : -1}">${icon(expanded ? 'chevron-down' : 'chevron-right')}${icon('git-branch')}<span class="pe-node-name">${escape(branch.branch_name)}${availability ? `<small>${escape(availability)}</small>` : ''}</span>${status(branch.lifecycle)}<code class="pe-node-sha">${escape(short(branch.head))}</code></button>${expanded ? `<ul role="group" class="pe-commits">${commits?.loading ? `<li role="none" class="pe-empty">${h('loadingCommits')}</li>` : commits?.error ? `<li role="none" class="pe-source-warning">${escape(commits.error)}<button type="button" class="btn btn-small" data-pe-retry-commits="${escape(branch.branch_id)}">${h('retry')}</button></li>` : commitRows || `<li role="none" class="pe-empty">${h('noCommits')}</li>`}${commits?.cursor ? `<li role="none"><button class="btn btn-small pe-more" type="button" data-pe-more-commits="${escape(branch.branch_id)}">${h('moreCommits')}</button></li>` : ''}</ul>` : ''}</li>`;
    }
    function commitUrl(branch, sha) {
      const url = external(repository(branch.repository_id)?.github_url);
      return url && /^[0-9a-f]{40,64}$/.test(sha || '') ? `${url.replace(/\/$/, '')}/commit/${sha}` : null;
    }
    function localTree() {
      const copies = s.data.checkouts;
      const kinds = [...new Set([...s.data.roots.map(r => r.kind), ...copies.map(c => c.root_kind || 'outside')])];
      return kinds.map(kind => {
        const projectRoot = s.data.roots.find(r => r.kind === kind);
        const current = copies.filter(c => (c.root_kind || 'outside') === kind);
        const entries = current.map(c => ({ id: c.worktree_id, path: c.relative_path, copy: c }));
        for (const copy of copies) for (const location of copy.locations || []) {
          if (location.root_kind === kind && location.relative_path && location.relative_path !== copy.relative_path) entries.push({ id: `${copy.worktree_id}:${location.relative_path}`, path: location.relative_path, copy, location });
        }
        const tree = pathTree(entries);
        return disclosure('root-' + kind, projectRoot?.label || t('outsideRoots'), renderPaths(tree, 2, kind), { trailing: projectRoot ? `<span class="pe-root-path">${escape(projectRoot.absolute_path || projectRoot.relative_path)}</span>` : '' });
      }).join('') || `<li role="none" class="pe-empty">${h('noCopies')}</li>`;
    }
    function pathTree(entries) {
      const tree = new Map();
      for (const entry of entries) {
        const segments = String(entry.path || t('locationUnknown')).replace(/^\.\//, '').split('/').filter(Boolean);
        let cursor = tree;
        for (let i = 0; i < segments.length; i++) {
          const label = segments[i];
          if (!cursor.has(label)) cursor.set(label, { label, children: new Map(), entries: [] });
          const node = cursor.get(label);
          if (i === segments.length - 1) node.entries.push(entry);
          cursor = node.children;
        }
      }
      return tree;
    }
    function renderPaths(tree, level, prefix) {
      return [...tree.values()].map(node => {
        const key = `${prefix}/${node.label}`;
        const rows = node.entries.map(entry => {
          const c = entry.copy;
          const linked = (c.branch_id && c.branch_id === s.branchId) || c.worktree_id === s.checkoutId;
          const actualState = c.exists === false ? c.lifecycle === 'removed' ? 'removed' : 'missing' : c.dirty === true ? 'dirty' : c.role === 'primary' ? 'primary' : c.lifecycle;
          return `<li role="none"><button class="pe-node pe-copy${linked ? ' linked' : ''}${c.worktree_id === s.checkoutId ? ' selected' : ''}" role="treeitem" aria-level="${level}" aria-selected="${linked}" type="button" tabindex="${linked ? 0 : -1}" data-node="copy-${escape(entry.id)}" data-pe-copy="${escape(c.worktree_id)}" data-pe-related="${escape(c.branch_id || '')}">${icon('database')}<span class="pe-node-name">${escape(node.label)}<small>${escape(t('role_' + c.role))}${entry.location ? ` · ${escape(t('buildOutput'))}` : c.head && !c.branch_id ? ` · ${escape(short(c.head))}` : ''}</small></span>${status(actualState)}<span class="pe-node-space">${escape(size(entry.location ? entry.location.allocated_bytes : c.source_bytes))}</span></button></li>`;
        }).join('');
        return rows + (node.children.size ? disclosure('path-' + key, node.label, renderPaths(node.children, level + 1, key), { level }) : '');
      }).join('');
    }
    function paintInspector() {
      if (s.edit) return;
      const panel = find('inspector');
      const item = selectedCopy() || selectedBranch();
      panel.hidden = !item || !['copies', 'branches'].includes(s.view);
      if (panel.hidden) return;
      const isCopy = Boolean(item.worktree_id), rights = permissions(item.repository_id);
      const title = isCopy ? item.relative_path : item.branch_name;
      const taskLink = item.task_id ? `#/plan/${encodeURIComponent(item.repository_id)}?task=${encodeURIComponent(item.task_id)}` : null;
      const pr = !isCopy && item.pr?.url ? external(item.pr.url) : null;
      panel.innerHTML = `<header>${icon(isCopy ? 'database' : 'git-branch')}<h2>${escape(title)}</h2>${status(item.lifecycle)}<div class="pe-inspector-links">${item.owner ? `<span>${h('owner')} <strong>${escape(item.owner)}</strong></span>` : ''}${taskLink ? `<a href="${taskLink}">${h('linkedOutcome')}${icon('external-link')}</a>` : ''}${pr ? `<a href="${escape(pr)}" target="_blank" rel="noopener noreferrer">PR #${escape(item.pr.number)}${icon('external-link')}</a>` : ''}</div><div class="pe-inspector-actions">${rights.edit ? `<button class="btn btn-primary" type="button" data-pe-edit>${icon('pencil')}${h('editDetails')}</button>` : ''}${rights.cleanup && (isCopy || item.worktree_ids.length) ? `<button class="btn" type="button" data-pe-remove>${icon('trash')}${h('reviewRemoval')}</button>` : ''}${!isCopy && rights.admin && item.merge_proof ? `<button class="btn" type="button" data-pe-retire>${h('retireBranch')}</button>` : ''}${isCopy && item.absolute_path ? `<button class="pe-icon-button" type="button" data-pe-copy-path aria-label="${escape(t('copyAbsolutePath'))}">${icon('copy')}</button>` : ''}</div></header>
        <dl class="pe-detail-fields"><div><dt>${h('intent')}</dt><dd>${escape(item.intent || t('notRecorded'))}</dd></div><div><dt>${h('outcome')}</dt><dd>${escape(item.outcome || t('notRecorded'))}</dd></div>${item.next_action ? `<div><dt>${h('nextAction')}</dt><dd>${escape(item.next_action)}</dd></div>` : ''}</dl>
        <details class="pe-facts"><summary>${h('localAndGitDetails')}</summary><dl><div><dt>${h('head')}</dt><dd><code>${escape(item.head || t('unknown'))}</code></dd></div><div><dt>${h('sourceSpace')}</dt><dd>${escape(size(item.source_bytes))}</dd></div><div><dt>${h('buildSpace')}</dt><dd>${escape(size(item.build_bytes))}</dd></div>${!isCopy && item.merge_proof ? `<div><dt>${h('integration')}</dt><dd>${escape(item.merge_proof.kind)} · ${escape(item.merge_proof.merged_into)}</dd></div>` : ''}</dl>${isCopy && item.protection_reasons.length ? `<p>${item.protection_reasons.map(reason => escape(reasonText(reason))).join(' · ')}</p>` : ''}</details>`;
    }
    function paintStorage(body) {
      const m = s.data.storage;
      body.innerHTML = `<section class="pe-storage"><h2>${h('localStorage')}</h2><dl class="pe-storage-totals">${[['sourceSpace', m.source_bytes], ['buildSpace', m.build_bytes], ['sharedSpace', m.shared_bytes], ['measuredLocalSpace', m.allocated_bytes]].map(([label, value]) => `<div><dt>${h(label)}</dt><dd>${escape(size(value))}</dd></div>`).join('')}</dl><p class="pe-muted">${h('unmeasuredCount', { count: m.unmeasured_count })} · ${h('measuredAt')} ${escape(date(m.measured_at))}</p><div class="pe-storage-table"><table><caption>${h('spaceByCopy')}</caption><thead><tr><th>${h('location')}</th><th>${h('lifecycle')}</th><th>${h('sourceSpace')}</th><th>${h('buildSpace')}</th></tr></thead><tbody>${s.data.checkouts.map(c => `<tr><td><button type="button" class="pe-text-button" data-pe-storage-copy="${escape(c.worktree_id)}">${escape(c.relative_path)}</button></td><td>${status(c.lifecycle)}</td><td>${escape(size(c.source_bytes))}</td><td>${escape(size(c.build_bytes))}</td></tr>`).join('')}</tbody></table></div>${s.data.next_checkout_cursor ? `<button class="btn" data-pe-more="checkouts">${h('moreCopies')}</button>` : ''}</section>`;
    }
    function paintHistory(body) {
      body.innerHTML = `<section class="pe-history"><h2>${h('history')}</h2><ol>${(s.events || s.data.activity).map(e => `<li><time datetime="${escape(e.at)}">${escape(date(e.at))}</time><p>${escape(e.summary)}</p></li>`).join('') || `<li class="pe-empty">${h('noHistory')}</li>`}</ol>${s.historyCursor ? `<button class="btn" data-pe-history-more>${h('earlierHistory')}</button>` : ''}</section>`;
    }
    function connect() {
      s.observer?.disconnect();
      const trees = find('trees'), links = find('links'); if (!trees || !links) return;
      const update = () => {
        if (!alive() || !links.isConnected) return;
        links.replaceChildren(); if (matchMedia('(max-width: 1050px)').matches) return;
        const from = trees.querySelector('.pe-branch.selected'); if (!from) return;
        const origin = trees.getBoundingClientRect(), a = from.getBoundingClientRect();
        links.setAttribute('viewBox', `0 0 ${origin.width} ${origin.height}`);
        for (const to of trees.querySelectorAll('.pe-copy.linked')) {
          const b = to.getBoundingClientRect();
          const x1 = a.right - origin.left, y1 = a.top + a.height / 2 - origin.top, x2 = b.left - origin.left, y2 = b.top + b.height / 2 - origin.top;
          const line = document.createElementNS('http://www.w3.org/2000/svg', 'path');
          line.setAttribute('d', `M${x1},${y1} C${x1 + 18},${y1} ${x2 - 18},${y2} ${x2},${y2}`); links.append(line);
        }
      };
      s.observer = new ResizeObserver(update); s.observer.observe(trees); requestAnimationFrame(update);
    }
    function treeKeyboard(event) {
      const node = event.target.closest('[role="treeitem"]'); if (!node) return;
      const tree = node.closest('[role="tree"]');
      const nodes = [...tree.querySelectorAll('[role="treeitem"]')].filter(n => n.getClientRects().length);
      const index = nodes.indexOf(node);
      let next;
      if (event.key === 'ArrowDown') next = nodes[Math.min(nodes.length - 1, index + 1)];
      if (event.key === 'ArrowUp') next = nodes[Math.max(0, index - 1)];
      if (event.key === 'Home') next = nodes[0];
      if (event.key === 'End') next = nodes.at(-1);
      if (['ArrowRight', 'ArrowLeft'].includes(event.key) && node.hasAttribute('aria-expanded')) {
        const open = node.getAttribute('aria-expanded') === 'true';
        if ((event.key === 'ArrowRight' && !open) || (event.key === 'ArrowLeft' && open)) { event.preventDefault(); node.click(); return; }
      }
      if (next) { event.preventDefault(); nodes.forEach(n => n.tabIndex = -1); next.tabIndex = 0; next.focus(); }
    }
    async function loadProjects(more = false) {
      const data = await api('project.list', { limit: 50, ...(more ? { after_project_id: s.projectCursor } : {}) });
      if (!alive()) return;
      s.projects = more ? [...s.projects, ...data.projects] : data.projects; s.projectCursor = data.next_project_id;
      paintProjects(); if (!s.projectId) paintBody();
    }
    async function loadStatus() {
      const read = ++s.read;
      let data;
      try { data = await api('project.status', { project_id: s.projectId }); }
      catch (error) {
        if (alive() && ['permission_denied', 'repository_not_found'].includes(error.code)) { snapshots.delete(cacheKey); s.data = null; paint(); }
        throw error;
      }
      if (!alive() || s.read !== read) return;
      const points = await Promise.allSettled([
        s.branchId && !data.branches.some(b => b.branch_id === s.branchId) ? api('project.branch.get', { project_id: s.projectId, branch_id: s.branchId }) : Promise.resolve(null),
        s.checkoutId && !data.checkouts.some(c => c.worktree_id === s.checkoutId) ? api('project.checkout.get', { project_id: s.projectId, worktree_id: s.checkoutId }) : Promise.resolve(null),
      ]);
      if (!alive() || s.read !== read) return;
      if (points[0].status === 'fulfilled' && points[0].value) data.branches.push(points[0].value);
      if (points[1].status === 'fulfilled' && points[1].value) data.checkouts.push(points[1].value);
      s.data = data; snapshots.set(cacheKey, data); s.error = '';
      if (s.branchId && !data.branches.some(b => b.branch_id === s.branchId)) {
        // Keep deep-link selections while later inventory pages load.
        if (!data.next_branch_cursor) s.branchId = '';
      }
      if (!s.branchId && !s.checkoutId) s.branchId = data.branches.find(b => b.lifecycle === 'active' || b.lifecycle === 'ready')?.branch_id || data.branches[0]?.branch_id || '';
      paint();
    }
    async function search(text) {
      const ticket = ++s.queryRead; s.query = text;
      const [branches, checkouts] = await Promise.all([
        api('project.branch.list', { project_id: s.projectId, query: text || null, limit: 50 }),
        api('project.checkout.list', { project_id: s.projectId, query: text || null, limit: 50 }),
      ]);
      if (!alive() || ticket !== s.queryRead) return;
      s.data = { ...s.data, branches: branches.branches, checkouts: checkouts.checkouts, next_branch_cursor: branches.next_cursor, next_checkout_cursor: checkouts.next_cursor };
      paintBody(); paintInspector();
    }
    async function more(kind) {
      const cursor = kind === 'branches' ? s.data.next_branch_cursor : s.data.next_checkout_cursor;
      if (!cursor) return;
      const page = await api(`project.${kind === 'branches' ? 'branch' : 'checkout'}.list`, { project_id: s.projectId, cursor, query: s.query || null, limit: 50 });
      if (!alive()) return;
      const id = kind === 'branches' ? 'branch_id' : 'worktree_id';
      s.data[kind] = [...new Map([...s.data[kind], ...page[kind]].map(item => [item[id], item])).values()];
      s.data[kind === 'branches' ? 'next_branch_cursor' : 'next_checkout_cursor'] = page.next_cursor;
      paintBody(); paintInspector();
    }
    async function loadCommits(branch, append = false) {
      if (!branch) return;
      const prev = s.commits.get(branch.branch_id); if (prev?.loading || (!append && prev?.items?.length && !prev.error)) return;
      s.commits.set(branch.branch_id, { items: append ? prev?.items || [] : [], loading: true }); paintBody();
      try {
        const params = { project_id: s.projectId, branch_id: branch.branch_id, cursor: append ? prev?.cursor : null, limit: 30 };
        let page = await api('project.branch.commits', params);
        if (page.refresh_required && alive()) {
          const job = await api('project.branch.commits.refresh', { project_id: s.projectId, branch_id: branch.branch_id, cursor: params.cursor, idempotency_key: key('commits') }, false);
          await watch(job);
          page = await api('project.branch.commits', params);
        }
        if (!alive()) return;
        s.commits.set(branch.branch_id, { items: [...(append ? prev?.items || [] : []), ...page.commits], cursor: page.next_cursor, loading: false });
      } catch (error) { if (alive()) s.commits.set(branch.branch_id, { items: prev?.items || [], error: error.message }); }
      paintBody();
    }
    async function refresh() {
      if (!s.projectId || s.refreshing) return;
      s.refreshing = true; s.error = ''; paintState();
      try {
        const job = await api('project.refresh', { project_id: s.projectId, include_remote: true, idempotency_key: key('refresh') }, false);
        if (!alive()) return;
        await watch(job);
        await loadStatus();
      } finally { s.refreshing = false; if (alive()) paintState(); }
    }
    async function watch(job) {
      s.job = job; let cursor = null;
      while (alive() && !finalJobs.has(s.job.state)) {
        const events = await api('event.wait', { cursor, filters: [{ filter_id: 'project-explorer', categories: ['other'], kinds: ['project.refreshed', 'project.refresh.completed', 'project.refresh.failed', 'project.updated', 'storage.job.finished', 'storage.job.failed'], deadline_at: new Date(Date.now() + 15000).toISOString() }] });
        if (!alive()) return;
        cursor = events.cursor;
        s.job = await api('project.job.status', { project_id: s.projectId, job_id: job.job_id });
      }
      if (alive() && s.job.error) throw new Error(reasonText(s.job.error));
    }
    async function historyPage(more = false) {
      const page = await api('project.history', { project_id: s.projectId, cursor: more ? s.historyCursor : null, limit: 30 });
      if (!alive()) return;
      s.events = more ? [...(s.events || []), ...page.events] : page.events; s.historyCursor = page.next_cursor;
      if (s.view === 'history') paintBody();
    }
    async function editDetails() {
      let item = selectedCopy() || selectedBranch(); if (!item || !permissions(item.repository_id).edit) return;
      const isCopy = Boolean(item.worktree_id), selectedId = item[isCopy ? 'worktree_id' : 'branch_id'];
      if (item.details_omitted) item = await api(`project.${isCopy ? 'checkout' : 'branch'}.get`, { project_id: s.projectId, [isCopy ? 'worktree_id' : 'branch_id']: selectedId });
      if (!alive() || selectedId !== (isCopy ? s.checkoutId : s.branchId)) return;
      const panel = document.createElement('dialog');
      panel.className = 'pe-metadata-dialog';
      panel.setAttribute('aria-label', t('editDetails'));
      panel.innerHTML = `<form data-pe-editor class="pe-editor" data-ui-region="metadata-editor"><header><h2>${h('editDetails')}</h2><span>${escape(isCopy ? item.relative_path : item.branch_name)}</span></header><div class="pe-edit-fields">${['intent', 'outcome', 'owner', 'task_id', 'next_action'].map(field => `<label>${h({ task_id: 'taskId', next_action: 'nextAction' }[field] || field)}${['intent', 'outcome', 'next_action'].includes(field) ? `<textarea name="${field}" rows="2" maxlength="2000">${escape(item[field] || '')}</textarea>` : `<input name="${field}" maxlength="200" value="${escape(item[field] || '')}">`}</label>`).join('')}<label>${h('lifecycle')}<select name="lifecycle">${[...new Set([item.lifecycle, ...editableStates])].map(value => `<option value="${escape(value)}"${value === item.lifecycle ? ' selected' : ''}${!editableStates.includes(value) ? ' disabled' : ''}>${escape(stateLabel(value))}</option>`).join('')}</select></label>${isCopy ? `<label>${h('role')}<select name="role"${['primary', 'deployment'].includes(item.role) ? ' disabled' : ''}>${[...new Set([item.role, 'worktree', 'clone', 'scratch'])].map(value => `<option value="${value}"${value === item.role ? ' selected' : ''}>${escape(t('role_' + value))}</option>`).join('')}</select></label>` : ''}</div><p data-pe-edit-error role="alert" hidden></p><footer><button class="btn" data-pe-cancel-edit type="button">${h('cancel')}</button><button class="btn btn-primary" type="submit">${h('saveChanges')}</button></footer></form>`;
      root.append(panel); s.edit = { item, isCopy };
      const close = () => { s.edit = null; panel.close(); panel.remove(); paintInspector(); find('inspector').querySelector('[data-pe-edit]')?.focus({preventScroll:true}); };
      panel.addEventListener('cancel', event => { event.preventDefault(); close(); });
      panel.showModal(); panel.querySelector('textarea').focus({ preventScroll: true });
      bindOutcomePicker(panel.querySelector('form'), item);
      panel.querySelector('[data-pe-cancel-edit]').onclick = close;
      panel.querySelector('form').addEventListener('submit', async event => {
        event.preventDefault(); const form = event.currentTarget, fields = new FormData(form); const payload = { project_id: s.projectId, [isCopy ? 'worktree_id' : 'branch_id']: item[isCopy ? 'worktree_id' : 'branch_id'], expected_revision: item.revision };
        for (const field of ['intent', 'outcome', 'owner', 'task_id', 'next_action']) if (String(fields.get(field) || '') !== String(item[field] || '')) payload[field] = String(fields.get(field) || '').trim() || null;
        if (fields.get('lifecycle') && fields.get('lifecycle') !== item.lifecycle) payload.lifecycle = fields.get('lifecycle');
        if (isCopy && fields.get('role') && fields.get('role') !== item.role) payload.role = fields.get('role');
        if (Object.keys(payload).length === 3) { close(); return; }
        form.querySelector('[type=submit]').disabled = true;
        try {
          const operation = `project.${isCopy ? 'checkout' : 'branch'}`;
          try { await api(`${operation}.update`, payload, false); }
          catch (failure) {
            if (!failure.code?.includes('conflict')) throw failure;
            const latest = await api(`${operation}.get`, { project_id: s.projectId, [isCopy ? 'worktree_id' : 'branch_id']: item[isCopy ? 'worktree_id' : 'branch_id'] });
            const changedFields = Object.keys(payload).filter(k => !['project_id','branch_id','worktree_id','expected_revision'].includes(k));
            if (changedFields.some(k => (latest[k] ?? null) !== (item[k] ?? null))) throw failure;
            await api(`${operation}.update`, { ...payload, expected_revision: latest.revision }, false);
          }
          close(); await loadStatus(); find('inspector').querySelector('[data-pe-edit]')?.focus({preventScroll:true});
        }
        catch (error) { if (alive()) { const message = form.querySelector('[data-pe-edit-error]'); message.hidden = false; message.textContent = error.message; form.querySelector('[type=submit]').disabled = false; } }
      });
    }
    function bindOutcomePicker(form, item) {
      const field = form.querySelector('[name=task_id]'), label = field.closest('label');
      field.type = 'hidden';
      const input = document.createElement('input'); input.type = 'search'; input.placeholder = t('searchOutcomes'); input.autocomplete = 'off'; input.setAttribute('aria-label', t('linkedOutcome'));
      const menu = document.createElement('div'); menu.className = 'pe-outcome-menu'; menu.dataset.uiContextualOverlay = 'Outcome search suggestions'; menu.setAttribute('role','listbox'); menu.id = `pe-outcomes-${crypto.randomUUID()}`; menu.hidden = true;
      input.setAttribute('role','combobox'); input.setAttribute('aria-autocomplete','list'); input.setAttribute('aria-controls',menu.id); input.setAttribute('aria-expanded','false');
      label.classList.add('pe-outcome-picker'); label.append(input,menu);
      let ticket = 0, timer;
      const hide = () => { menu.hidden=true; input.setAttribute('aria-expanded','false'); };
      if (item.task_id) {
        input.value = t('currentOutcome');
        void api('task.history',{task_id:item.task_id}).then(result=>{if(form.isConnected && input.value===t('currentOutcome')) input.value=result.task.title;}).catch(()=>{});
      }
      input.addEventListener('input',()=>{
        clearTimeout(timer);const current=++ticket;const query=input.value.trim();
        if(!query){field.value='';input.setCustomValidity('');hide();return;}
        input.setCustomValidity(t('chooseOutcomeResult'));
        timer=setTimeout(async()=>{
          if(!alive() || !form.isConnected)return;
          try {
            const result=await api('task.search',{repository_id:item.repository_id,query,limit:12});
            if(!alive()||!form.isConnected||current!==ticket)return;
            menu.replaceChildren();
            for(const task of result.tasks){const choice=document.createElement('button');choice.type='button';choice.setAttribute('role','option');choice.textContent=`#${task.sequence} · ${task.title}`;choice.addEventListener('click',()=>{field.value=task.task_id;input.value=task.title;input.setCustomValidity('');hide();input.focus();});menu.append(choice);}
            if(!result.tasks.length){const empty=document.createElement('p');empty.textContent=t('noMatchingOutcomes');menu.append(empty);}
            menu.hidden=false;input.setAttribute('aria-expanded','true');
          }catch(error){if(form.isConnected&&current===ticket){menu.textContent=error.message;menu.hidden=false;}}
        },200);
      });
      input.addEventListener('keydown',event=>{if(event.key==='ArrowDown'&&!menu.hidden){event.preventDefault();menu.querySelector('button')?.focus();}if(event.key==='Escape'&&!menu.hidden){event.preventDefault();event.stopPropagation();hide();}});
      menu.addEventListener('keydown',event=>{const choices=[...menu.querySelectorAll('button')],index=choices.indexOf(document.activeElement);if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();choices[(index+(event.key==='ArrowDown'?1:-1)+choices.length)%choices.length]?.focus();}if(event.key==='Escape'){event.preventDefault();event.stopPropagation();hide();input.focus();}});
      form.addEventListener('focusout',()=>{queueMicrotask(()=>{if(!label.contains(document.activeElement))hide();});});
    }
    async function onClick(event) {
      if (!alive()) return;
      const button = event.target.closest('button,a'); if (!button || !root.contains(button)) return;
      if (button.dataset.peView) { event.preventDefault(); s.view = button.dataset.peView; s.edit = null; route(); paint(); if (s.view === 'history') await action(() => historyPage()); }
      if (button.dataset.peDisclose) { const id = button.dataset.peDisclose; s.collapsed.has(id) ? s.collapsed.delete(id) : s.collapsed.add(id); paintBody(); }
      if (button.dataset.peBranch) { const id = button.dataset.peBranch; if (s.showCommits) { if (s.branchId === id && s.open.has(id)) s.open.delete(id); else s.open.add(id); } s.branchId = id; s.checkoutId = ''; s.edit = null; route(); paintBody(); paintInspector(); if (s.showCommits && s.open.has(id)) await loadCommits(selectedBranch()); }
      if (button.dataset.peCopy || button.dataset.peStorageCopy) { s.checkoutId = button.dataset.peCopy || button.dataset.peStorageCopy; const copy = selectedCopy(); s.branchId = copy?.branch_id || ''; if (button.dataset.peStorageCopy) s.view = 'copies'; s.edit = null; route(); paint(); }
      if (button.hasAttribute('data-pe-edit')) await action(editDetails);
      if (button.dataset.peMore) { button.disabled = true; await action(() => more(button.dataset.peMore)); if (button.isConnected) button.disabled = false; }
      if (button.dataset.peMoreCommits || button.dataset.peRetryCommits) { const id = button.dataset.peMoreCommits || button.dataset.peRetryCommits; await loadCommits(s.data.branches.find(b => b.branch_id === id), Boolean(button.dataset.peMoreCommits)); }
      if (button.hasAttribute('data-pe-history-more')) await action(() => historyPage(true));
      if (button.hasAttribute('data-pe-copy-path')) await action(async () => { await navigator.clipboard.writeText(selectedCopy().absolute_path); button.setAttribute('aria-label', t('pathCopied')); });
      if (button.hasAttribute('data-pe-remove')) await action(() => removalPlan());
      if (button.hasAttribute('data-pe-retire')) await action(() => retireBranch());
      if (button.hasAttribute('data-pe-cancel-removal') && s.removalJob) await action(async () => { s.removalJob = await api('project.checkout.remove.cancel', { project_id: s.projectId, job_id: s.removalJob.job.job_id }, false); paintRemovalJob(); });
    }

    // The lifecycle service owns loss classification, target identity and receipts.
    async function removalPlan() {
      const copy = selectedCopy(), branch = selectedBranch();
      const ids = copy ? [copy.worktree_id] : branch?.worktree_ids || [];
      if (!ids.length) return;
      const targets = ids.map(worktree_id => s.data.checkouts.find(c => c.worktree_id === worktree_id)).filter(Boolean);
      if (targets.length !== ids.length) { throw new Error(t('loadCopiesBeforeRemoval')); }
      const plan = await api('project.checkout.remove.plan', { project_id: s.projectId, expected_project_revision: s.data.revision, targets: targets.map(c => ({ worktree_id: c.worktree_id, expected_revision: c.revision })) }, false);
      if (!alive()) return;
      await showRemoval(plan);
    }
    async function showRemoval(plan) {
      const approvalKey = key('approve'), removalKey = key('remove');
      const dialog = document.createElement('dialog'); dialog.className = 'pe-removal-dialog';
      dialog.setAttribute('aria-labelledby', 'pe-removal-title');
      const items = plan.targets || [];
      const canApprove = plan.requires_approval && s.data.permissions.admin && !plan.blockers.length;
      dialog.innerHTML = `<header><h2 id="pe-removal-title">${h('reviewRemoval')}</h2><button class="pe-icon-button" type="button" data-close aria-label="${escape(t('close'))}">${icon('x')}</button></header><p>${escape(plan.effect || t('removalEffect'))}</p><ul class="pe-removal-targets">${items.map(item => `<li><strong>${escape(item.path)}</strong><span>${escape(size(item.estimated_reclaimable_bytes))}</span><small>${escape(item.branch_name || t('detached'))} · ${escape(short(item.head))}</small><dl class="pe-removal-facts"><div><dt>${h('remotePreservation')}</dt><dd>${h(item.remote_recoverable ? 'verified' : 'notVerified')}</dd></div><div><dt>${h('untrackedFiles')}</dt><dd>${item.untracked_files}</dd></div><div><dt>${h('ignoredFiles')}</dt><dd>${item.ignored_files}</dd></div><div><dt>${h('stashEntries')}</dt><dd>${item.stash_entries}</dd></div></dl>${[...item.active_uses, ...item.protections, ...item.blockers].length ? `<p>${[...new Set([...item.active_uses, ...item.protections, ...item.blockers])].map(r => escape(reasonText(r))).join(' · ')}</p>` : ''}${item.possible_data_loss.length ? `<p class="pe-data-loss"><strong>${h('dataWouldBeLost')}</strong> ${item.possible_data_loss.map(r => escape(reasonText(r))).join(' · ')}</p>` : ''}${item.task_stays_open ? `<p>${h('taskRemainsOpen')}</p>` : ''}</li>`).join('')}</ul><p>${h('estimatedReclaim')} <strong>${escape(size(plan.estimated_reclaimable_bytes))}</strong></p>${plan.blockers.length ? `<p class="pe-data-loss">${plan.blockers.map(r => escape(reasonText(r))).join(' · ')}</p>` : ''}${plan.requires_approval && !canApprove ? `<p>${h('adminApprovalNeeded')}</p>` : ''}<p data-removal-error role="alert" hidden></p><footer><button class="btn" type="button" data-close>${h('cancel')}</button><button class="btn btn-danger" type="button" data-start${!plan.ready && !canApprove ? ' disabled' : ''}>${h(canApprove ? 'approveLossAndRemove' : 'removeSelectedCopies')}</button></footer>`;
      root.append(dialog); const close = () => { dialog.close(); dialog.remove(); find('inspector').querySelector('[data-pe-remove]')?.focus(); };
      dialog.querySelectorAll('[data-close]').forEach(b => b.onclick = close); dialog.addEventListener('cancel', event => { event.preventDefault(); close(); });
      dialog.querySelector('[data-start]').onclick = async event => {
        event.currentTarget.disabled = true;
        try {
          let approval;
          if (canApprove) approval = await api('project.checkout.remove.approve', { project_id: s.projectId, plan_id: plan.plan_id, fingerprint: plan.fingerprint, idempotency_key: approvalKey }, false);
          const job = await api('project.checkout.remove.start', { project_id: s.projectId, plan_id: plan.plan_id, fingerprint: plan.fingerprint, approval_receipt_id: approval?.receipt_id || null, idempotency_key: removalKey }, false);
          close(); await watchRemoval(job); await loadStatus();
        }
        catch (error) { if (dialog.isConnected) { const box = dialog.querySelector('[data-removal-error]'); box.hidden = false; box.textContent = error.message; event.target.disabled = false; } else fault(error); }
      };
      dialog.showModal();
    }
    function paintRemovalJob() {
      if (!alive() || !s.removalJob) return;
      const box = find('job'), {job, targets} = s.removalJob; box.hidden = false;
      const finished = finalJobs.has(job.state);
      const complete = finished && targets.length > 0 && targets.every(target => target.fully_removed);
      box.innerHTML = `<span>${h(complete ? 'removalVerified' : finished ? 'removalIncomplete' : 'removalRunning')} · ${escape(size(job.reclaimed_bytes))}</span>${!finished ? `<button class="btn btn-small" data-pe-cancel-removal>${h('cancelRemaining')}</button>` : ''}${job.error_code ? `<span>${escape(reasonText(job.error_code))}</span>` : ''}`;
    }
    async function watchRemoval(job) {
      s.removalJob = job; paintRemovalJob(); let cursor = null;
      try { sessionStorage.setItem(pendingRemovalKey, JSON.stringify({ project_id: s.projectId, job_id: job.job.job_id })); } catch {}
      while (alive() && !finalJobs.has(s.removalJob.job.state)) {
        const events = await api('event.wait', { cursor, filters: [{ filter_id: 'project-removal', categories: ['other'], kinds: ['storage.job.finished','storage.job.failed','storage.job.progress'], deadline_at: new Date(Date.now()+15000).toISOString() }] });
        if (!alive()) return; cursor = events.cursor;
        s.removalJob = await api('project.checkout.remove.job', { project_id: s.projectId, job_id: job.job.job_id }); paintRemovalJob();
      }
      if (alive() && finalJobs.has(s.removalJob.job.state)) { try { sessionStorage.removeItem(pendingRemovalKey); } catch {} }
    }
    async function retireBranch() {
      const branch = selectedBranch(); if (!branch || !permissions(branch.repository_id).admin) return;
      if (!branch.cleanup_receipt || !branch.pr || !branch.head) throw new Error(t('completeLocalCleanupFirst'));
      const intent = `${branch.branch_id}:${branch.head}`;
      if (!s.retireKeys.has(intent)) s.retireKeys.set(intent, key('retire'));
      const job = await api('project.branch.retire', { project_id: s.projectId, branch_id: branch.branch_id, expected_revision: branch.revision, expected_head: branch.head, cleanup_job_id: branch.cleanup_receipt.job_id, worktree_id: branch.cleanup_receipt.worktree_id, pull_request_number: branch.pr.number, integration_branch: branch.merge_proof?.merged_into || null, idempotency_key: s.retireKeys.get(intent) }, false);
      await watch(job); await loadStatus();
    }
    const projectsRead = action(loadProjects);
    if (s.projectId) {
      if (snapshots.has(cacheKey)) { s.data = snapshots.get(cacheKey); paint(); }
      await action(loadStatus);
      let pending; try { pending = JSON.parse(sessionStorage.getItem(pendingRemovalKey) || 'null'); } catch {}
      if (alive() && pending?.project_id === s.projectId && typeof pending.job_id === 'string') {
        void action(async () => { const job = await api('project.checkout.remove.job', pending); await watchRemoval(job); await loadStatus(); });
      }
      if (alive() && s.data) void action(refresh);
    } else paint();
    await projectsRead;
  }
  return { mount, href };
})();
