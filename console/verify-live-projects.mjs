// The native self-validation runner supplies the actual project/branch IDs.
// This observes the installed Console; it never creates disposable live copies.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import {createRequire} from 'node:module';

export function hasFreshGithubObservation(repository, now=Date.now()) {
  const observation=repository.github_observation;
  const observed=Date.parse(observation?.last_success||'');
  return Boolean(repository.github_url && observation?.state==='fresh' && observation.job_id && Number.isFinite(observed) && observed<=now && now-observed<=10*60*1000);
}

export async function verifyLiveProjects({root,out,base,request,check,checks}) {
  const projectId=process.env.CONSOLE_VERIFY_PROJECT_ID;
  const branchId=process.env.CONSOLE_VERIFY_BRANCH_ID;
  const commit=process.env.CONSOLE_VERIFY_SOURCE_COMMIT;
  assert.match(projectId||'',/^[a-zA-Z0-9_.:-]{1,128}$/);
  assert.match(branchId||'',/^[a-zA-Z0-9_.:-]{1,128}$/);
  assert.match(commit||'',/^[a-f0-9]{40}$/);
  const call=async(operation,params)=>{
    const response=await request(`/api/v3/${operation}`,JSON.stringify(params));
    assert.equal(response.status,200,operation);const reply=JSON.parse(response.body);assert.equal(reply.ok,true,`${operation} failed`);return reply.data;
  };
  const ping=await call('ping',{});check('Installed daemon matches the merged source',ping.source_commit===commit);
  const project=await call('project.status',{project_id:projectId});
  const branch=await call('project.branch.get',{project_id:projectId,branch_id:branchId});
  check('The real project owns the selected branch',project.repositories.some(r=>r.repository_id===branch.repository_id));
  const github=project.repositories.filter(r=>hasFreshGithubObservation(r));
  check('The project has a recent complete live GitHub branch observation',github.length>0);
  const {chromium}=createRequire(path.join(process.env.CONSOLE_VERIFY_PLAYWRIGHT||path.join(root,'ci/playwright'),'package.json'))('playwright');
  const browser=await chromium.launch({args:[`--host-resolver-rules=MAP ${base.hostname} 127.0.0.1`]});
  try {
    const page=await browser.newPage({viewport:{width:1487,height:1058},colorScheme:'dark'});page.setDefaultTimeout(30000);
    const errors=[];page.on('pageerror',error=>errors.push(error.name));
    const route=`${base.href}#/projects/${projectId}?view=copies&branch=${branchId}`;
    await page.goto(route);await page.locator(`[data-pe-branch="${branchId}"]`).waitFor();
    check('Live project title is visible',await page.locator('[data-pe-title]').innerText()===project.project.name);
    check('Both real inventory trees are visible',await page.locator('.pe-tree-panel').count()===2);
    check('Selected branch intent is read from stored data',(await page.locator('[data-pe-inspector]').innerText()).includes(branch.branch_name));
    await page.locator('[data-pe-edit]').click();
    const owner=page.locator('[data-pe-editor] [name=owner]');
    check('Metadata editor contains the current owner',await owner.inputValue()===(branch.owner||''));
    // Opening/cancelling is read-only. Real persistence is separately exercised
    // on this task's branch metadata during authorized registry adoption.
    await page.locator('[data-pe-cancel-edit]').click();await page.locator('[data-pe-editor]').waitFor({state:'detached'});
    await page.locator('[data-pe-show-commits]').check();await page.locator('.pe-commit').first().waitFor();
    check('Selected live branch exposes commit history',await page.locator('.pe-commit').count()>0);
    for(const view of ['overview','storage','history']) {
      await page.locator(`[data-pe-view=${view}]`).click();await page.locator(`.pe-${view}`).waitFor();
      check(`${view} has real visible content`,(await page.locator(`.pe-${view}`).innerText()).length>0);
    }
    await page.goto(route);await page.locator(`[data-pe-branch="${branchId}"]`).waitFor();
    const primary=project.checkouts.find(c=>c.role==='primary');
    if(primary){
      await page.goto(`${base.href}#/projects/${projectId}?checkout=${primary.worktree_id}`);
      await page.locator('[data-pe-remove]').click();await page.locator('.pe-removal-dialog[data-removal-result]').waitFor();
      check('Retained primary has no enabled removal',await page.locator('.pe-removal-dialog [data-start]:enabled').count()===0);
      await page.locator('.pe-removal-dialog footer [data-close]').click();
    }
    await page.setViewportSize({width:390,height:844});await page.goto(route);await page.locator(`[data-pe-branch="${branchId}"]`).waitFor();
    check('Live phone layout fits the viewport',await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1));
    check('Live Console has no browser exceptions',errors.length===0);
    await fs.writeFile(path.join(out,'project-observation.json'),JSON.stringify({schema:1,result:'passed',project_id:projectId,branch_id:branchId,source_commit:commit,github_observations:github.map(r=>({repository_id:r.repository_id,...r.github_observation})),checked_at_ms:Date.now(),checks:checks.map(c=>({name:c.name,passed:c.passed}))}),{mode:0o600,flag:'wx'});
  } finally {await browser.close();}
}
