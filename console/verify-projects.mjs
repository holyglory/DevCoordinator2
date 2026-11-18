// Extends the existing real edge/ControlPlane/PostgreSQL browser harness.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';
import { explorerFormalConfig } from './project-explorer-formal.mjs';
import { hasFreshGithubObservation } from './verify-live-projects.mjs';
const execute = promisify(execFile);

export async function verifyProjects({page,browser,base,call,request,check,cookieFor,out,root,temporary}) {
  page.setDefaultTimeout(8000);
  const fixture=await call('fixture.projects.context');
  const id=fixture.project_id, route=`${base}#/projects/${encodeURIComponent(id)}`;
  const branch=()=>page.locator('[data-pe-branch]').filter({hasText:'codex/clock-divider-drag'});
  await check('Live handoff requires a recent complete GitHub observation independent of local availability',async()=>{
    const now=Date.now(),last_success=new Date(now).toISOString();
    const fresh={github_url:'https://github.com/fixture-owner/fixture-project',refresh_state:'failed',error:'Local snapshot unavailable',github_observation:{job_id:'fixture-refresh',state:'fresh',last_success}};
    assert.equal(hasFreshGithubObservation(fresh,now),true);
    assert.equal(hasFreshGithubObservation(fresh,now),true);
    for(const observation of [null,{...fresh.github_observation,state:'failed'},{...fresh.github_observation,job_id:null},{...fresh.github_observation,last_success:'invalid'},{...fresh.github_observation,last_success:new Date(now-11*60*1000).toISOString()},{...fresh.github_observation,last_success:new Date(now+1000).toISOString()}]) {
      assert.equal(hasFreshGithubObservation({...fresh,refresh_state:'fresh',error:null,last_success,github_observation:observation},now),false);
    }
  });
  await check('Project lists preserve explicit product membership',async()=>{
    const result=await call('project.list',{},'owner@example.test');
    assert.equal(result.projects.filter(p=>p.project_id===id).length,1);
    assert.equal(result.projects.find(p=>p.project_id===id).repository_count,2);
    const other=await call('project.list',{},'outsider@example.test');
    assert.equal(other.projects.some(p=>p.project_id===id),false);
  });
  await check('Explorer renders real branches and copy relationships',async()=>{
    await page.goto(route); await branch().waitFor();
    await branch().click();
    assert.match(await page.locator('[data-pe-title]').innerText(),/HDL Ripper/);
    assert.equal(await page.locator('.pe-tree-panel').count(),2);
    assert.ok(await page.locator('.pe-copy.linked').count()>=1);
    assert.match(await page.locator('[data-pe-inspector]').innerText(),/Dragging preserves connections/);
    assert.equal(await page.locator('.pe-copy').filter({hasText:'Primary'}).count()>=1,true);
    const state=await call('project.status',{project_id:id});
    assert.equal(state.checkouts.find(c=>c.worktree_id===fixture.feature_worktree_id).dirty,false);
    assert.ok(state.checkouts.some(c=>c.dirty===true));
    // Every folder leaf represents an observed checkout or its declared
    // artifact location; deployment references must never invent paths.
    const copyIds=await page.locator('.pe-copy').evaluateAll(rows=>rows.map(row=>row.dataset.peCopy));
    assert.ok(copyIds.every(copyId=>state.checkouts.some(c=>c.worktree_id===copyId)));
    await page.locator('.pe-project[aria-current=page]').waitFor();
  },page);
  await check('Projects connect host navigation to the existing Plan',async()=>{
    await page.locator('[data-pe-context-links] a[href^="#/plan/"]').click();
    await page.locator('.plan-context').waitFor();
    assert.match(await page.locator('#workspace-heading').innerText(),/HDL Ripper/);
    await page.locator('#nav-toggle').click();
    await page.locator('#nav a[data-view=health]').click();
    await page.locator('#nav-toggle').click();
    await page.locator('#nav a[data-view=projects]').click();
    await page.locator(`.pe-project-index a[href="#/projects/${id}?view=copies"]`).click();
    await branch().waitFor();await branch().click();
    assert.match(await page.locator('[data-pe-title]').innerText(),/HDL Ripper/);
  },page);
  await check('Branch metadata editing persists and reloads',async()=>{
    await page.locator('[data-pe-edit]').click();
    await page.locator('[data-pe-editor] [name=intent]').fill('Keep every clock-divider connection while dragging');
    await page.locator('[data-pe-editor] [name=owner]').fill('Explorer operator');
    await page.locator('[data-pe-editor] [type=submit]').click();
    await page.locator('[data-pe-editor]').waitFor({state:'detached'});
    const result=await call('project.status',{project_id:id});
    const row=result.branches.find(b=>b.branch_name==='codex/clock-divider-drag');
    assert.equal(row.intent,'Keep every clock-divider connection while dragging');
    assert.equal(row.owner,'Explorer operator');
    await page.reload(); await branch().waitFor();await branch().click();
    await page.getByText('Keep every clock-divider connection while dragging',{exact:true}).waitFor();
  },page);
  await check('Concurrent edits cannot silently overwrite intent',async()=>{
    const data=await call('project.status',{project_id:id});const b=data.branches.find(b=>b.branch_name==='codex/clock-divider-drag');
    await call('project.branch.update',{project_id:id,branch_id:b.branch_id,expected_revision:b.revision,next_action:'Review the drag interaction'});
    const denied=await request('project.branch.update',{project_id:id,branch_id:b.branch_id,expected_revision:b.revision,intent:'Stale draft'},'owner@example.test');
    assert.equal(denied.ok,false);assert.match(denied.error.code,/conflict/);
  });
  await check('Selecting a branch loads its complete stored description',async()=>{
    const data=await call('project.status',{project_id:id});const row=data.branches.find(b=>b.branch_name==='codex/clock-divider-drag');
    const full='Preserve every attached connection and label while the clock divider moves. '.repeat(18)+' Complete intent ends here.';
    await call('project.branch.update',{project_id:id,branch_id:row.branch_id,expected_revision:row.revision,intent:full});
    try {
      const list=await call('project.branch.list',{project_id:id,limit:50});
      assert.equal(list.branches.find(b=>b.branch_id===row.branch_id).details_omitted,true);
      await page.reload();await branch().waitFor();await branch().click();
      await page.locator('[data-pe-inspector]').getByText(full,{exact:true}).waitFor();
    } finally {
      const current=await call('project.branch.get',{project_id:id,branch_id:row.branch_id});
      await call('project.branch.update',{project_id:id,branch_id:row.branch_id,expected_revision:current.revision,intent:row.intent});
      await page.reload();await branch().waitFor();await branch().click();
    }
  },page);
  await check('Commit disclosure comes from actual Git history',async()=>{
    if(await page.locator('[data-pe-cancel-edit]').isVisible()) await page.locator('[data-pe-cancel-edit]').click();
    await page.locator('[data-pe-show-commits]').check();
    await page.locator('.pe-commit').first().waitFor();
    assert.match(await page.locator('.pe-commits').innerText(),/Preserve connections while dragging/);
    await fs.writeFile(path.join(fixture.feature,'continued-work.txt'),'The selected branch continues after the first observation.\n');
    await execute('git',['-C',fixture.feature,'add','continued-work.txt']);
    await execute('git',['-C',fixture.feature,'-c','user.name=Explorer fixture','-c','user.email=fixture@example.test','commit','-qm','Continue the selected branch']);
    await execute('git',['-C',fixture.feature,'push','-q','origin','HEAD']);
    await page.locator('[data-pe-refresh]:not([disabled])').waitFor();await page.locator('[data-pe-refresh]').click();
    await page.locator('.pe-commit').filter({hasText:'Continue the selected branch'}).waitFor({timeout:30000});
  },page);
  await check('A background refresh preserves the active branch and path search',async()=>{
    await page.locator('[data-pe-search] input').fill('clock-divider');await page.locator('[data-pe-search] button').click();
    await branch().waitFor();
    await page.locator('[data-pe-refresh]:not([disabled])').waitFor();await page.locator('[data-pe-refresh]').click();
    await page.locator('[data-pe-refresh]:not([disabled])').waitFor({timeout:30000});
    assert.ok((await page.locator('[data-pe-branch]').allTextContents()).every(text=>text.includes('clock-divider')));
    await page.locator('[data-pe-search] input').fill('');await page.locator('[data-pe-search] button').click();
    await page.locator('[data-pe-branch]').filter({hasText:'waveform-export'}).waitFor();
  },page);
  await check('Source and build space keeps missing values explicit',async()=>{
    await page.locator('[data-pe-view=storage]').click();
    await page.locator('.pe-storage').waitFor();
    assert.match(await page.locator('.pe-storage').innerText(),/Not measured/);
    const data=await call('project.status',{project_id:id});
    assert.ok(data.storage.unmeasured_count>0);
  },page);
  await check('Phone storage keeps its caption readable and labels every amount',async()=>{
    try {
      await page.setViewportSize({width:390,height:664});
      for(const theme of ['light','dark']) {
        await page.evaluate(theme=>{document.documentElement.dataset.theme=theme;localStorage.setItem('dc2-theme',theme);},theme);
        const facts=await page.locator('.pe-storage-table').evaluate(table=>{
          const caption=table.querySelector('caption'),style=getComputedStyle(caption);
          return {captionHeight:caption.getBoundingClientRect().height,lineHeight:parseFloat(style.lineHeight)||parseFloat(style.fontSize)*1.5,rows:[...table.querySelectorAll('tbody tr')].map(row=>row.innerText)};
        });
        assert.ok(facts.captionHeight<=facts.lineHeight*2+1&&facts.rows.length>0&&facts.rows.every(row=>/Source\s*(?:[\d.,]+\s*[KMGT]?i?B|Not measured)/.test(row)&&/Builds\s*(?:[\d.,]+\s*[KMGT]?i?B|Not measured)/.test(row)),JSON.stringify({theme,...facts}));
      }
    } finally {await page.setViewportSize({width:1440,height:900});}
  },page);
  await check('Project history records actual metadata changes',async()=>{
    await page.locator('[data-pe-view=history]').click();
    await page.locator('.pe-history li').first().waitFor();
    const history=await call('project.history',{project_id:id,limit:50});assert.ok(history.events.length>0);
  },page);
  await check('Operators edit only authorized project metadata',async()=>{
    const data=await call('project.status',{project_id:id},'operator@example.test');
    const b=data.branches.find(b=>b.branch_name==='codex/clock-divider-drag');
    const result=await call('project.branch.update',{project_id:id,branch_id:b.branch_id,expected_revision:b.revision,owner:'Operator'},'operator@example.test');
    assert.equal(result.owner,'Operator');
    const context=await browser.newContext({viewport:{width:1440,height:1024}});
    try {await context.addCookies([cookieFor('operator@example.test')]);const p=await context.newPage();await p.goto(route);await p.locator('[data-pe-branch]').first().waitFor();await p.locator('[data-pe-branch]').filter({hasText:'codex/clock-divider-drag'}).click();assert.equal(await p.locator('[data-pe-edit]').isVisible(),true);assert.equal(await p.locator('[data-pe-copy-path]').count(),0);}finally{await context.close();}
  });
  await check('Viewer reads exclude absolute paths and mutations',async()=>{
    const data=await call('project.status',{project_id:id},'viewer@example.test');
    assert.equal(data.permissions.edit,false);
    assert.ok(data.checkouts.every(c=>!Object.hasOwn(c,'absolute_path')));
    assert.ok(data.roots.every(c=>!Object.hasOwn(c,'absolute_path')));
    const b=data.branches[0];const rejected=await request('project.branch.update',{project_id:id,branch_id:b.branch_id,expected_revision:b.revision,owner:'Not allowed'},'viewer@example.test');assert.equal(rejected.ok,false);
    const context=await browser.newContext({viewport:{width:390,height:844}});
    try {await context.addCookies([cookieFor('viewer@example.test')]);const p=await context.newPage();await p.goto(route);await p.locator('[data-pe-branch]').first().waitFor();await p.locator('[data-pe-branch]').first().click();assert.equal(await p.locator('[data-pe-edit]').count(),0);assert.equal(await p.locator('[data-pe-remove]').count(),0);}finally{await context.close();}
  });
  await check('Primary local edits remain protected from removal',async()=>{
    const data=await call('project.status',{project_id:id});const primary=data.checkouts.find(c=>c.role==='primary'&&c.absolute_path===fixture.primary);assert.ok(primary);
    const params={project_id:id,expected_project_revision:data.revision,targets:[{worktree_id:primary.worktree_id,expected_revision:primary.revision}],idempotency_key:'browser-primary-removal-review'};
    const result=await request('project.checkout.remove.plan',params,'operator@example.test');
    if (result.ok) {
      let review=result.data;
      const retry=await call('project.checkout.remove.plan',params,'operator@example.test');assert.equal(retry.job.job_id,review.job.job_id);
      const deadline=Date.now()+30000;
      while(['queued','running','cancelling'].includes(review.job.state)&&Date.now()<deadline){
        await call('event.wait',{cursor:null,filters:[{filter_id:'project-browser-review',categories:['other'],kinds:['storage.job.progress','storage.job.finished','storage.job.failed'],deadline_at:new Date(Date.now()+1000).toISOString()}]},'operator@example.test');
        review=await call('project.checkout.remove.plan.status',{project_id:id,job_id:review.job.job_id},'operator@example.test');
      }
      assert.ok(!['queued','running','cancelling'].includes(review.job.state),'Review job did not finish');
      if(review.plan){
        assert.equal(review.plan.ready,false);
        assert.ok([...review.plan.blockers,...review.plan.targets.flatMap(t=>t.blockers)].some(reason=>/primary|canonical|protected|linked_worktrees/.test(reason)),JSON.stringify(review.plan));
      }else{assert.equal(review.job.state,'failed');assert.match(review.job.error_code,/primary|canonical|clone_has_linked_worktrees/);}
    } else {
      assert.equal(result.error.code,'storage_blocked',JSON.stringify(result.error));
      assert.match(result.error.message,/primary|canonical|clone_has_linked_worktrees/);
    }
    assert.equal((await fs.readFile(path.join(fixture.primary,'unsaved-notes.txt'),'utf8')).includes('must survive'),true);
    await page.goto(route); await page.locator(`[data-pe-copy="${primary.worktree_id}"]`).waitFor();
    await page.locator(`[data-pe-copy="${primary.worktree_id}"]`).click();
    await page.locator('[data-pe-remove]').click();
    const dialog=page.locator('.pe-removal-dialog[data-removal-result]'); await dialog.waitFor();
    assert.match(await dialog.innerText(),/primary|canonical|protected|linked.worktrees/i);
    assert.equal(await dialog.locator('[data-start]:enabled').count(),0);
    await dialog.locator('footer [data-close]').click();
  });
  await check('Reloading a pending review restores exactly one dialog and one job',async()=>{
    const data=await call('project.status',{project_id:id});const primary=data.checkouts.find(c=>c.role==='primary'&&c.absolute_path===fixture.primary);
    let release;const held=new Promise(resolve=>{release=resolve;});
    const holdEvents=async request=>{await held;await request.continue().catch(()=>{});};
    await page.route('**/api/v3/event.wait',holdEvents);
    try {
      await page.goto(`${route}?checkout=${primary.worktree_id}`);await page.locator('[data-pe-remove]').waitFor();
      const requested=page.waitForResponse(response=>response.url().endsWith('/api/v3/project.checkout.remove.plan'));
      await page.locator('[data-pe-remove]').click();const queued=await (await requested).json();assert.equal(queued.ok,true);
      await page.waitForFunction(jobId=>Object.keys(sessionStorage).some(key=>key.startsWith('dc2-project-review:')&&JSON.parse(sessionStorage.getItem(key)).job_id===jobId),queued.data.job.job_id);
      await page.reload();await page.locator('.pe-removal-dialog[open]').waitFor();
      assert.equal(await page.locator('.pe-removal-dialog').count(),1);release();
      await page.locator('.pe-removal-dialog[data-removal-result]').waitFor();
      assert.equal(await page.locator('.pe-removal-dialog [data-start]:enabled').count(),0);
      await page.locator('.pe-removal-dialog footer [data-close]').click();
      await page.locator('[data-pe-remove]').click();await page.locator('.pe-removal-dialog[data-removal-result]').waitFor();
      assert.equal(await page.locator('.pe-removal-dialog').count(),1);
      await page.locator('.pe-removal-dialog footer [data-close]').click();
    } finally {release();await page.unroute('**/api/v3/event.wait',holdEvents);}
  },page);
  if (fixture.native) {
    await check('Operator removes a remotely preserved temporary checkout through Console',async()=>{
      const context=await browser.newContext({viewport:{width:1487,height:1058}});
      try {
        await context.addCookies([cookieFor('operator@example.test')]);const p=await context.newPage();p.setDefaultTimeout(30000);
        await p.goto(`${route}?checkout=${fixture.safe_worktree_id}`);await p.locator('[data-pe-remove]').click();
        await p.locator('.pe-removal-dialog[data-removal-result=ready]').waitFor();
        const started=p.waitForResponse(r=>r.url().endsWith('/api/v3/project.checkout.remove.start'));
        await p.locator('.pe-removal-dialog [data-start]').click();const reply=await(await started).json();assert.equal(reply.ok,true);
        await p.locator('[data-pe-job]').filter({hasText:'Removal verified'}).waitFor();
        await assert.rejects(fs.stat(fixture.safe),{code:'ENOENT'});
        const result=await call('project.checkout.remove.job',{project_id:id,job_id:reply.data.job.job_id},'operator@example.test');
        assert.equal(result.targets.find(t=>t.worktree_id===fixture.safe_worktree_id).fully_removed,true);
        assert.ok(await fs.stat(fixture.primary));
      } finally {await context.close();}
    });
    await check('Dirty local data requires the explicit administrator approval shown in Console',async()=>{
      await page.goto(`${route}?checkout=${fixture.dirty_worktree_id}`);await page.locator('[data-pe-remove]').click();
      const dialog=page.locator('.pe-removal-dialog[data-removal-result=review]');await dialog.waitFor();
      assert.match(await dialog.innerText(),/Local data may be lost/);
      await page.locator('.pe-removal-dialog footer [data-close]').click();assert.ok(await fs.stat(fixture.dirty));
      await page.locator('[data-pe-remove]').click();await dialog.waitFor();
      const approved=page.waitForResponse(r=>r.url().endsWith('/api/v3/project.checkout.remove.approve'));
      const started=page.waitForResponse(r=>r.url().endsWith('/api/v3/project.checkout.remove.start'));
      await page.getByRole('button',{name:'Discard the listed data and remove',exact:true}).click();
      const approval=await(await approved).json(),job=await(await started).json();assert.equal(approval.ok,true);assert.equal(job.ok,true);
      await page.locator('[data-pe-job]').filter({hasText:'Removal verified'}).waitFor({timeout:30000});
      await assert.rejects(fs.stat(fixture.dirty),{code:'ENOENT'});
      const result=await call('project.checkout.remove.job',{project_id:id,job_id:job.data.job.job_id});
      assert.equal(result.targets.find(t=>t.worktree_id===fixture.dirty_worktree_id).fully_removed,true);
      assert.match(await fs.readFile(path.join(fixture.primary,'unsaved-notes.txt'),'utf8'),/must survive/);
    },page);
  }
  for (const theme of ['light','dark']) for(const width of [390,760,1050,1440,1487]) await check(`Project explorer ${theme} ${width}: visible tree and no horizontal page overflow`,async()=>{
    await page.setViewportSize({width,height:width===1487?1058:900});await page.emulateMedia({colorScheme:theme});
    await page.goto(`${route}?view=copies`);await branch().waitFor();await branch().click();
    await page.evaluate(theme=>{document.documentElement.dataset.theme=theme;localStorage.setItem('dc2-theme',theme);},theme);
    assert.ok(await page.locator('.pe-remote').isVisible());assert.ok(await page.locator('.pe-local').isVisible());
    const size=await page.evaluate(()=>({width:innerWidth,scroll:document.documentElement.scrollWidth}));assert.ok(size.scroll<=size.width+1,JSON.stringify(size));
    await page.screenshot({path:path.join(out,`explorer-${theme}-${width}.png`),fullPage:true,mask:[page.locator('#who-email')]});
  },page);
  await check('Keyboard tree navigation keeps selection accessible',async()=>{
    await page.setViewportSize({width:1440,height:1024});await page.goto(route);await branch().waitFor();
    for(const selector of ['.pe-remote','.pe-local']) {
      await page.locator(`${selector} [role=treeitem]`).first().focus();
      for(const key of ['ArrowDown','End','Home']) {
        await page.keyboard.press(key);
        const focused=page.locator(`${selector} [role=treeitem]:focus`);assert.equal(await focused.count(),1);
        const bounds=await focused.evaluate(node=>({row:node.getBoundingClientRect().toJSON(),pane:node.closest('[role=tree]').getBoundingClientRect().toJSON()}));
        assert.ok(bounds.row.top>=bounds.pane.top-1&&bounds.row.bottom<=bounds.pane.bottom+1,JSON.stringify({selector,key,...bounds}));
      }
    }
  },page);
  await check('Phone editing reaches Save and Cancel without losing the entered metadata',async()=>{
    await page.setViewportSize({width:390,height:664});await page.goto(route);await branch().waitFor();await branch().click();
    await page.locator('[data-pe-edit]').click();
    const owner=page.locator('[data-pe-editor] [name=owner]');await owner.fill('Phone editor verification');
    const save=page.locator('[data-pe-editor] [type=submit]');await save.focus();await save.click({trial:true});await save.click();
    await page.locator('[data-pe-editor]').waitFor({state:'detached'});
    await page.locator('[data-pe-inspector]').getByText('Phone editor verification',{exact:true}).waitFor();
    await page.locator('[data-pe-edit]').click();
    await owner.fill('Discard this phone draft');
    const cancel=page.locator('[data-pe-cancel-edit]');await cancel.focus();await cancel.click({trial:true});await cancel.click();
    await page.locator('[data-pe-editor]').waitFor({state:'detached'});
    await page.locator('[data-pe-inspector]').getByText('Phone editor verification',{exact:true}).waitFor();
    assert.equal(await page.locator('[data-pe-inspector]').getByText('Discard this phone draft',{exact:true}).count(),0);
  },page);
  if(process.env.PROJECT_EXPLORER_FORMAL_BINARY) await check('Project explorer fresh formal light and dark coverage',async()=>{
    const binary=process.env.PROJECT_EXPLORER_FORMAL_BINARY;const pw=process.env.CONSOLE_VERIFY_PLAYWRIGHT||'/opt/holyskills-validation-runtime';
    const status=await call('project.status',{project_id:id});const branchId=status.branches.find(b=>b.branch_name==='codex/clock-divider-drag').branch_id;
    const index=await page.request.get(base);assert.equal(index.status(),200);
    assert.ok((await index.body()).equals(await fs.readFile(path.join(root,'console/index.html'))));
    const sourceBinding={expected:index.headers().etag,responseHeader:'etag',metaName:null};
    assert.ok(sourceBinding.expected,'Served Console must expose its source identity');
    const failures=[];
    for(const theme of ['light','dark']) {
      const config=explorerFormalConfig({root,base,projectId:id,branchId,theme,playwrightModuleDir:path.join(pw,'node_modules'),cookie:`dc2_session=${cookieFor('owner@example.test').value}`,sourceBinding});
      const files=config.targets[0].reviewInputs.map(input=>input.path);
      const css=await fs.readFile(path.join(root,'console/project-explorer.css'),'utf8');
      for(const match of css.matchAll(/url\('\/(icons\/tabler\/[^']+)'\)/g))files.push(`console/${match[1]}`);
      const observed=[];
      for(const file of new Set(files)) {
        if(!(await fs.stat(path.join(root,file))).isFile())continue;
        const response=await page.request.get(new URL(file.slice('console/'.length),base).href),bytes=await response.body();
        assert.equal(response.status(),200,file);assert.ok(bytes.equals(await fs.readFile(path.join(root,file))),file);
        observed.push({path:file,sha256:createHash('sha256').update(bytes).digest('hex')});
      }
      await fs.writeFile(path.join(out,`served-ui-inputs-${theme}.json`),JSON.stringify({sourceBinding,files:observed},null,2));
      const configFile=path.join(temporary,`projects-formal-${theme}.json`);await fs.writeFile(configFile,JSON.stringify(config),{mode:0o600});
      const dir=path.join(out,`formal-${theme}`);await fs.mkdir(dir,{recursive:true});
      let result;try{result=await execute(binary,['formal-ui','verify','--config',configFile,'--json-out',path.join(dir,'report.json'),'--markdown-out',path.join(dir,'report.md'),'--screenshot-dir',path.join(dir,'screenshots')],{cwd:root,maxBuffer:1024*1024});}catch(error){await fs.writeFile(path.join(dir,'receipt.json'),error.stdout||'');await fs.writeFile(path.join(dir,'failure.log'),error.stderr||error.message);failures.push(theme);continue;}
      await fs.writeFile(path.join(dir,'receipt.json'),result.stdout);const receipt=JSON.parse(result.stdout);if(receipt.formal?.result!=='passed')failures.push(theme);
    }
    assert.deepEqual(failures,[],`Formal verification did not pass for: ${failures.join(', ')}`);
  },page);
}
