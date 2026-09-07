'use strict';

window.DevCoordinatorWorkspace = (() => {
  const repositoryViews = new Set(['plan', 'progress', 'usage', 'deployments', 'tests', 'decisions', 'glossary']);
  const workViews = new Set(['plan', 'progress', 'usage']);

  function catalogue(repositories, runs, deployments) {
    const records = new Map();
    const add = (row) => {
      if (!row.repository_id) return;
      const previous = records.get(row.repository_id);
      records.set(row.repository_id, {
        ...previous, ...row,
        display_name: previous?.display_name || row.display_name || 'Repository',
        paths: [...new Set([...(previous?.paths || []), row.root_path, row.worktree_path].filter(Boolean))],
      });
    };
    repositories.forEach(add);
    runs.forEach(add);
    deployments.forEach((deployment) => add({ repository_id: deployment.repository_id, display_name: deployment.repository_name }));
    const groups = new Map();
    for (const record of records.values()) {
      const key = record.repository_source?.key || record.repository_id;
      if (!groups.has(key)) groups.set(key, { key, name: record.repository_source?.name || record.display_name, records: [] });
      groups.get(key).records.push(record);
    }
    for (const group of groups.values()) {
      group.records.sort((left, right) => Number(right.display_name === group.name) - Number(left.display_name === group.name)
        || left.display_name.localeCompare(right.display_name) || left.repository_id.localeCompare(right.repository_id));
      group.repositoryId = group.records[0].repository_id;
      group.paths = [...new Set(group.records.flatMap((record) => record.paths))];
    }
    return [...groups.values()].sort((left, right) => left.name.localeCompare(right.name) || left.key.localeCompare(right.key));
  }

  function href(view, repositoryId) {
    return ['tests', 'deployments'].includes(view)
      ? `#/${view}?repository=${encodeURIComponent(repositoryId)}`
      : `#/${view}/${encodeURIComponent(repositoryId)}`;
  }

  function create({ api, esc, identity }) {
    const shell = document.querySelector('#repository-workspace');
    const sidebar = document.querySelector('#repository-sidebar');
    const navigation = document.querySelector('#repository-list');
    const heading = document.querySelector('#workspace-heading');
    const aspects = document.querySelector('#workspace-aspects');
    const workNavigation = document.querySelector('#workspace-work-views');
    const toggle = document.querySelector('#repository-toggle');
    const overlay = document.querySelector('#repository-overlay');
    const refreshButton = document.querySelector('#repository-refresh');
    const search = document.querySelector('#repository-search');
    let data;
    let groups = [];
    let selectedId = '';
    let currentView = 'plan';
    let active = false;
    let loading;
    let searchText = '';
    let remembered = '';
    try { remembered = sessionStorage.getItem('dc2-workspace-repository') || ''; } catch {}

    const groupFor = (repositoryId) => groups.find((group) => group.records.some((record) => record.repository_id === repositoryId));
    const current = () => groupFor(selectedId);
    const canOperate = () => identity()?.administrator || Object.values(identity()?.grants || {}).some((role) => ['operator', 'administrator'].includes(role));

    function closeDrawer(restoreFocus = false) {
      shell.classList.remove('repository-drawer-open');
      toggle.setAttribute('aria-expanded', 'false');
      overlay.hidden = true;
      if (restoreFocus) toggle.focus();
    }

    function paintNavigation() {
      const visible = groups.filter((group) => [group.name, ...group.paths].some((value) => value.toLowerCase().includes(searchText.toLowerCase())));
      navigation.innerHTML = visible.length ? visible.map((group) => {
        const selected = group === current();
        const duplicateName = groups.filter((item) => item.name === group.name).length > 1;
        return `<a class="workspace-repository" href="${href(currentView, selected ? selectedId : group.repositoryId)}"${selected ? ' aria-current="page"' : ''}><svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 7V5a1 1 0 0 1 1-1h5l3 3h8a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7Z"/></svg><span>${esc(group.name)}${duplicateName ? `<small>${esc(group.paths[0] || group.records[0].display_name)}</small>` : ''}</span></a>`;
      }).join('') : `<p class="muted">${searchText ? 'No matching repositories.' : 'No repositories available.'}</p>`;
      const selected = current();
      const checkouts = document.querySelector('#workspace-checkouts');
      checkouts.hidden = !selected?.paths.length;
      checkouts.innerHTML = selected ? `<summary>Checkouts${selected.paths.length > 1 ? ` <span>${selected.paths.length}</span>` : ''}</summary><div>${selected.records.map((record) => `<div class="workspace-checkout">${record.paths.map((path) => `<span>${esc(path)}</span>`).join('')}${selected.records.length > 1 ? `<a href="${href(currentView, record.repository_id)}"${selectedId === record.repository_id ? ' aria-current="page"' : ''}>${selectedId === record.repository_id ? 'Selected' : 'Use this checkout’s plan & records'}</a>` : ''}</div>`).join('')}</div>` : '';
    }

    function paint() {
      shell.classList.toggle('repository-scoped', active);
      sidebar.hidden = !active;
      document.querySelector('#workspace-context').hidden = !active;
      closeDrawer();
      if (!active) { workNavigation.hidden = true; return; }
      const selected = current();
      heading.textContent = selected?.name || 'Repositories';
      toggle.setAttribute('aria-label', `Choose repository${selected ? `. Current repository: ${selected.name}` : ''}`);
      const record = selected?.records.find((item) => item.repository_id === selectedId);
      const scope = document.querySelector('#workspace-record-scope');
      scope.hidden = !record || selectedId === selected?.repositoryId || ['tests', 'deployments'].includes(currentView);
      scope.textContent = scope.hidden ? '' : record.paths[0] || record.display_name;
      const tabs = [['plan', 'Plan & progress'], ['deployments', 'Deployments'], ['tests', 'Tests'], ['decisions', 'Decisions'], ['glossary', 'Glossary']];
      aspects.innerHTML = selectedId ? tabs.map(([view, label]) => `<a href="${href(view, selectedId)}"${(view === currentView || view === 'plan' && workViews.has(currentView)) ? ' aria-current="page"' : ''}>${label}</a>`).join('') : '';
      workNavigation.hidden = !selectedId || !workViews.has(currentView);
      workNavigation.innerHTML = selectedId && workViews.has(currentView) ? [['plan', 'Plan'], ...(canOperate() ? [['progress', 'Progress'], ['usage', 'Usage']] : [])].map(([view, label]) => `<a href="${href(view, selectedId)}"${view === currentView ? ' aria-current="page"' : ''}>${label}</a>`).join('') : '';
      paintNavigation();
    }

    async function load(signal) {
      if (data) return;
      if (loading?.signal === signal) return loading.promise;
      const read = async (operation) => {
        try { return { value: await api(operation, operation === 'usage.repositories' ? { range: '24h' } : {}) }; }
        catch (error) { if (signal.aborted || error.code === 'stale' || error.code === 'unauthenticated') throw error; return { error }; }
      };
      const promise = Promise.all([read('plan.overview'), read('test.list'), read('deployment.list'), ...(canOperate() ? [read('usage.repositories'), read('progress.repositories')] : [])]).then(([plans, tests, deploymentList, usage, progress]) => {
        if (signal.aborted) return;
        if ([plans, tests, deploymentList, usage, progress].filter(Boolean).every((result) => result.error)) throw plans.error;
        data = { repositories: [...(plans.value?.repositories || []), ...(usage?.value?.repositories || []), ...(progress?.value?.repositories || [])], runs: tests.value?.runs || [], deployments: deploymentList.value?.deployments || [] };
        groups = catalogue(data.repositories, data.runs, data.deployments);
        const unavailable = [['Plan', plans], ['Tests', tests], ['Deployments', deploymentList]].filter(([, result]) => result.error && result.error.code !== 'permission_denied');
        document.querySelector('#repository-status').textContent = unavailable.length ? `${unavailable.map(([label]) => label).join(', ')} repositories unavailable. Refresh to retry.` : '';
      });
      loading = { signal, promise };
      return promise;
    }

    async function resolve(signal) {
      const [pathname, queryString = ''] = (location.hash || '#/plan').split('?');
      const [, view = 'plan', argument = ''] = pathname.split('/');
      currentView = view;
      active = repositoryViews.has(view) && !(view === 'glossary' && (!argument || argument === 'shared'));
      if (!active) { paint(); return { view, arg: argument }; }
      shell.classList.add('repository-scoped');
      sidebar.hidden = false;
      document.querySelector('#workspace-context').hidden = false;
      if (!data) navigation.innerHTML = '<p class="muted">Loading repositories…</p>';
      await load(signal);
      if (signal.aborted) return null;
      const query = new URLSearchParams(queryString);
      let requested = ['plan', 'progress', 'usage', 'decisions', 'glossary'].includes(view) ? argument : query.get('repository');
      if (view === 'tests' && argument) requested = data.runs.find((run) => run.run_id === argument || run.earlier_visual_evidence?.run_id === argument)?.repository_id || requested;
      if (view === 'deployments' && argument) requested = data.deployments.find((deployment) => deployment.deployment_id === argument)?.repository_id || requested;
      if (['tests', 'deployments'].includes(view) && argument && !requested) {
        active = false; selectedId = ''; paint();
        return { view, arg: `${argument}${view === 'tests' && queryString ? `?${queryString}` : ''}` };
      }
      selectedId = requested || (groupFor(remembered) ? remembered : groups[0]?.repositoryId) || '';
      if (requested && !groupFor(requested)) {
        const unmatchedDetail = ['tests', 'deployments'].includes(view) && argument;
        if (unmatchedDetail) { active = false; selectedId = ''; }
        else {
          groups.push({ key: requested, name: 'Repository unavailable', repositoryId: requested, paths: [], records: [{ repository_id: requested, display_name: 'Repository unavailable', paths: [] }] });
        }
      }
      if (selectedId) {
        remembered = selectedId;
        try { sessionStorage.setItem('dc2-workspace-repository', selectedId); } catch {}
        if (!argument && !query.has('repository')) {
          const target = href(view, selectedId);
          const separator = target.includes('?') ? '&' : '?';
          window.history.replaceState(null, '', `${target}${queryString ? `${separator}${queryString}` : ''}`);
        }
      }
      paint();
      return { view, arg: view === 'tests' ? argument && `${argument}${queryString ? `?${queryString}` : ''}` : view === 'deployments' ? argument : argument || selectedId, empty: !selectedId && !argument, settings: query.get('settings') };
    }

    toggle.addEventListener('click', () => {
      if (shell.classList.contains('repository-drawer-open')) { closeDrawer(true); return; }
      shell.classList.add('repository-drawer-open');
      toggle.setAttribute('aria-expanded', 'true');
      overlay.hidden = false;
      search.focus();
    });
    overlay.addEventListener('click', () => closeDrawer(true));
    document.querySelector('#repository-close').addEventListener('click', () => closeDrawer(true));
    sidebar.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') { event.preventDefault(); closeDrawer(true); }
      if (event.key === 'Tab' && shell.classList.contains('repository-drawer-open')) {
        const controls = [...sidebar.querySelectorAll('a,button,input,summary')].filter((element) => element.getClientRects().length);
        const first = controls[0]; const last = controls.at(-1);
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
      }
    });
    sidebar.addEventListener('click', (event) => { if (event.target.closest('a')) closeDrawer(true); });
    search.addEventListener('input', () => { searchText = search.value; paintNavigation(); });
    refreshButton.addEventListener('click', async () => {
      refreshButton.disabled = true;
      data = null;
      try { await window.render(); } finally { refreshButton.disabled = false; }
    });
    return { resolve, href, current, get active() { return active; }, get repositoryId() { return selectedId; },
      matches: (row) => current()?.records.some((record) => record.repository_id === row.repository_id) || false };
  }
  return { catalogue, href, create };
})();
