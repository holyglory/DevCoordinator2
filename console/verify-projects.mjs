// Extends the existing real edge/ControlPlane/PostgreSQL browser harness.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { explorerFormalConfig } from './project-explorer-formal.mjs';
const execute = promisify(execFile);

export async function verifyProjects({page,browser,base,call,request,check,cookieFor,out,root,temporary}) {
  page.setDefaultTimeout(8000);
  const fixture=await call('fixture.projects.context');
  const id=fixture.project_id, route=`${base}#/projects/${encodeURIComponent(id)}`;
  const branch=()=>page.locator('[data-pe-branch]').filter({hasText:'codex/clock-divider-drag'});
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
  await check('Commit disclosure comes from actual Git history',async()=>{
    if(await page.locator('[data-pe-cancel-edit]').isVisible()) await page.locator('[data-pe-cancel-edit]').click();
    await page.locator('[data-pe-show-commits]').check();
    await page.locator('.pe-commit').first().waitFor();
    assert.match(await page.locator('.pe-commits').innerText(),/Preserve connections while dragging/);
  },page);
  await check('Source and build space keeps missing values explicit',async()=>{
    await page.locator('[data-pe-view=storage]').click();
    await page.locator('.pe-storage').waitFor();
    assert.match(await page.locator('.pe-storage').innerText(),/Not measured/);
    const data=await call('project.status',{project_id:id});
    assert.ok(data.storage.unmeasured_count>0);
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
    const data=await call('project.status',{project_id:id});const primary=data.checkouts.find(c=>c.role==='primary');assert.ok(primary);
    const result=await request('project.checkout.remove.plan',{project_id:id,expected_project_revision:data.revision,targets:[{worktree_id:primary.worktree_id,expected_revision:primary.revision}]},'operator@example.test');
    if (result.ok) {
      assert.equal(result.data.ready,false);
      assert.ok([...result.data.blockers,...result.data.targets.flatMap(t=>t.blockers)].some(reason=>/primary|canonical|protected|linked_worktrees/.test(reason)),JSON.stringify(result.data));
    } else {
      assert.equal(result.error.code,'storage_blocked');
      assert.match(result.error.message,/primary|canonical|clone_has_linked_worktrees/);
    }
    assert.equal((await fs.readFile(path.join(fixture.primary,'unsaved-notes.txt'),'utf8')).includes('must survive'),true);
  });
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
    const first=page.locator('.pe-remote [role=treeitem]').first();await first.focus();await page.keyboard.press('ArrowDown');
    assert.ok(await page.locator('.pe-remote [role=treeitem]:focus').count());
    await page.keyboard.press('End');assert.ok(await page.locator('.pe-remote [role=treeitem]:focus').count());
  },page);
  if(process.env.PROJECT_EXPLORER_FORMAL_BINARY) await check('Project explorer fresh formal light and dark coverage',async()=>{
    const binary=process.env.PROJECT_EXPLORER_FORMAL_BINARY;const pw=process.env.CONSOLE_VERIFY_PLAYWRIGHT||'/opt/holyskills-validation-runtime';
    for(const theme of ['light','dark']) {
      const config=explorerFormalConfig({root,base,projectId:id,theme,playwrightModuleDir:path.join(pw,'node_modules'),cookie:`dc2_session=${cookieFor('owner@example.test').value}`});
      const configFile=path.join(temporary,`projects-formal-${theme}.json`);await fs.writeFile(configFile,JSON.stringify(config),{mode:0o600});
      const dir=path.join(out,`formal-${theme}`);await fs.mkdir(dir,{recursive:true});
      let result;try{result=await execute(binary,['formal-ui','verify','--config',configFile,'--json-out',path.join(dir,'report.json'),'--markdown-out',path.join(dir,'report.md'),'--screenshot-dir',path.join(dir,'screenshots')],{cwd:root,maxBuffer:1024*1024});}catch(error){await fs.writeFile(path.join(dir,'receipt.json'),error.stdout||'');throw error;}
      await fs.writeFile(path.join(dir,'receipt.json'),result.stdout);const receipt=JSON.parse(result.stdout);assert.equal(receipt.formal.result,'passed');
    }
  },page);
}
