// Source-owned formal contract for the real, isolated Agent hub candidate.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { createHash } from 'node:crypto';

const geometry=(primary,heading)=>[
  {id:'content-width',kind:'primary-content-width',selector:primary,minWidth:275},
  {id:'heading',kind:'readable-heading',selector:heading},
  {id:'canonical-identity',kind:'readable-canonical-identifier',selector:heading},
  {id:'word-wrapping',kind:'no-character-wrapping',selector:heading},
  {id:'document-width',kind:'document-horizontal-overflow',selector:'html'},
  {id:'initial-placement',kind:'initial-viewport-placement',selector:primary},
  {id:'content-clipping',kind:'clipping',selector:primary},
];

export function agentHubFormalConfig({root,base,theme,cookie,sourceBinding,playwrightModuleDir,roomId,emptyRoomId,publicRoomId,zeroRoomId,fileMessageId,firstRun=false,role='administrator'}) {
  const reviewInputs=['console/agent-hub.js','console/agent-hub.css','console/app.js','console/index.html','console/app.css','console/design-system.css','console/locales/en/agent-hub.json'].map(file=>({path:file,kind:file.endsWith('.css')?'style':'ui-code'}));
  const journey={id:'agent-handoff',name:'Connect an agent and inspect its real communication handoff',frequencyPercent:100,risk:'high',rationale:'The Agent hub guides an administrator from access setup to durable delivery evidence.'};
  const target=(name,step,query='',shapeSelector='.hub-panel[aria-busy="false"]')=>({
    name:`hub-${name}-${theme}`,url:`${base}#/agent-hub?step=${step}${query}`,theme,sourceBinding,
    authProfile:role,
    reviewEnvironmentIdentity:'isolated-agent-hub-console-candidate',
    journeys:[journey],primaryJourney:journey.id,
    regions:[{name:'Current handoff step',selector:'.hub-panel',role:'primary-content',journey:journey.id}],
    geometryAssertions:geometry('.hub-panel','#hub-panel-title'),
    waitFor:{selector:shapeSelector==='.hub-panel[aria-busy="false"]'?shapeSelector:`.hub-panel[aria-busy="false"] ${shapeSelector}`,renderFrames:2,timeoutMs:10000},reviewInputs,
    execution:{parallelSafe:true},
    fixtureDataShapes:[{id:name,revision:'real-agent-hub-candidate-v1',conditionalDom:[shapeSelector],layoutEffect:'Actual persisted fixture agents, rooms, messages and receipt rows populate this state through the real Console API.'}],
    states:[],
  });
  const key=target('key','key');
  key.states.push({name:'rotation-confirmation',actions:[{action:'click',selector:'[data-hub-action="generate-key"]'}],waitFor:{selector:'.hub-confirm',renderFrames:2},continuation:{kind:'in-page',anchor:'[data-hub-action="confirm-rotate"]',focusWithin:'.hub-confirm',maxScrollDelta:8},execution:{parallelSafe:true,resourceLocks:['hub-settings']},fixtureDataShapes:[{id:'rotation-confirmation',revision:'real-agent-hub-candidate-v1',conditionalDom:['.hub-confirm'],layoutEffect:'The actual existing-key rotation control asks for a focused confirmation before changing the fixture key.'}],cleanup:{actions:[{action:'click',selector:'[data-hub-action="cancel-rotate"]'}]}});
  key.states.push({name:'public-settings',actions:[{action:'click',selector:'[data-hub-action="public-settings"]'}],waitFor:{selector:'.hub-settings',renderFrames:2},continuation:{kind:'in-page',anchor:'.hub-settings h3',focusWithin:'.hub-settings',maxScrollDelta:8},fixtureDataShapes:[{id:'public-settings',revision:'real-agent-hub-candidate-v1',conditionalDom:['.hub-settings'],layoutEffect:'Actual persisted public enabled and paused flags expose the administrator controls.'}]});
  key.states.push({name:'one-time-reveal',actions:[{action:'click',selector:'[data-hub-action="generate-key"]'},{action:'click',selector:'[data-hub-action="confirm-rotate"]'}],waitFor:{selector:'[data-hub-key-value]',renderFrames:2},continuation:{kind:'in-page',anchor:'[data-hub-key-value]',focusWithin:'[data-hub-secret]',maxScrollDelta:8},execution:{parallelSafe:true,resourceLocks:['hub-settings']},fixtureDataShapes:[{id:'masked-key-reveal',revision:'real-agent-hub-candidate-v1',conditionalDom:['[data-hub-secret]','[data-hub-key-value]'],layoutEffect:'The candidate generates a new fixture-only key and renders its one-time reveal; screenshot masking excludes the entire reveal.'}],cleanup:{actions:[{action:'click',selector:'[data-hub-action="dismiss-key"]'}]}});
  if(role==='viewer')key.states=[];
  const connect=target('connect','connect');
  connect.states.push({name:'remote-client',actions:[{action:'selectOption',selector:'#hub-client',value:'remote'}],continuation:{kind:'in-page',anchor:'#hub-client',focusWithin:'.hub-panel',maxScrollDelta:8},waitFor:{selector:'#hub-client',renderFrames:2}});
  connect.states.push({name:'public-client',actions:[{action:'selectOption',selector:'#hub-client',value:'public'}],continuation:{kind:'in-page',anchor:'#hub-client',focusWithin:'.hub-panel',maxScrollDelta:8},waitFor:{selector:'#hub-client',renderFrames:2}});
  const rooms=target('rooms','rooms','', '[data-hub-room]');
  rooms.fixtureDataShapes[0].conditionalDom.push('[data-hub-agent] .hub-presence.is-active','[data-hub-agent] .hub-presence:not(.is-active)');
  const publicRooms=target('public-rooms','rooms','&space=public','[data-hub-room]');
  const delivery=target('delivery','delivery',`&room=${roomId}`,'[data-hub-delivery]');
  delivery.states.push({name:'attachment-preview',actions:[{action:'click',selector:`[data-hub-delivery="${fileMessageId}"] .hub-files li:first-child [data-hub-action="preview"]`}],regions:[{name:'Safe attachment preview',selector:'dialog.hub-preview[open]',role:'primary-content',journey:journey.id}],geometryAssertions:geometry('dialog.hub-preview[open]','dialog.hub-preview[open] h3'),waitFor:{selector:'dialog.hub-preview[open] pre',renderFrames:2},continuation:{kind:'in-page',anchor:'dialog.hub-preview[open] h3',focusWithin:'dialog.hub-preview[open]',maxScrollDelta:8},fixtureDataShapes:[{id:'attachment-text-preview',revision:'real-agent-hub-candidate-v1',conditionalDom:['dialog.hub-preview[open] pre'],layoutEffect:'A verified attachment is fetched from the actual candidate service and displayed as escaped text.'}],cleanup:{actions:[{action:'click',selector:'[data-hub-action="close-preview"]'}]}});
  const empty=target('empty-room','delivery',`&room=${emptyRoomId}`,'.hub-conversation .hub-empty');
  const zero=target('zero-recipients','delivery',`&room=${zeroRoomId}`,'[data-hub-delivery]');
  const publicDelivery=target('public-delivery','delivery',`&space=public&room=${publicRoomId}`,'[data-hub-delivery]');
  // Shared shell/type changes are sampled on one real populated rooms layout.
  // Transient credential/dialog states retain the actual phone and desktop
  // samples without multiplying an unrelated room-column transition.
  const boundaryRooms={...rooms,name:`hub-layout-boundaries-${theme}`,states:[],breakpointProfile:{name:'hub-layout',breakpoints:[720,1100,1500],height:1058,baseViewport:'desktop'}};
  const targets=firstRun?[{...key,name:`hub-first-run-${theme}`,states:[],fixtureDataShapes:[{id:'no-key-first-run',revision:'real-agent-hub-candidate-v1',conditionalDom:['[data-hub-action="generate-key"]'],layoutEffect:'A fresh isolated authority with no saved instance key renders the actual first-run Generate action.'}]}]:[key,connect,rooms,publicRooms,delivery,empty,zero,publicDelivery,...(role==='administrator'?[boundaryRooms]:[])];
  const viewports=[{name:'phone',device:'iPhone 13',colorScheme:theme},...(role==='administrator'?[{name:'intermediate',width:900,height:1024,colorScheme:theme}]:[]),{name:'desktop',width:1487,height:1058,colorScheme:theme},...(role==='administrator'?[{name:'wide',width:1920,height:1080,colorScheme:theme}]:[])];
  const requiredCoverage=targets.flatMap(t=>['base',...t.states.map(s=>s.name)].flatMap(state=>viewports.map(viewport=>({target:t.name,state,viewport:viewport.name}))));
  if(!firstRun&&role==='administrator')for(const breakpoint of [720,1100,1500])for(const offset of [-1,0,1])requiredCoverage.push({target:boundaryRooms.name,state:'base',viewport:`hub-layout-${breakpoint}-${offset<0?'minus-1':offset>0?'plus-1':'at'}`,width:breakpoint+offset});
  const fixtureDataShapes=targets.flatMap(t=>[{name:'base',fixtureDataShapes:t.fixtureDataShapes},...t.states].flatMap(s=>(s.fixtureDataShapes||t.fixtureDataShapes).map(shape=>({...shape,id:`${t.name}-${s.name}-${shape.id}`,target:t.name,route:new URL(t.url).pathname+new URL(t.url).search,state:s.name}))));
  return {repoRoot:root,playwrightModuleDir,cookies:[cookie],authProfiles:[{name:role,url:targets[0].url,actions:[{action:'focus',selector:'#hub-panel-title'}],waitFor:{selector:'.hub-panel[aria-busy="false"]',renderFrames:2,timeoutMs:10000}}],targets,viewports,requiredCoverage,fixtureDataShapes,maxPageCount:256,screenshotMasks:[{selector:'[data-hub-secret]',reason:'The fixture instance key is a one-time credential and must never enter retained images'},{selector:'#who-email',reason:'Console identity is private'},{selector:'#workspace-root',reason:'Local filesystem paths are private'}]};
}

export async function verifyAgentHubFormal(options) {
  const {base,root,out,temporary,browser,cookieFor}=options;
  const probe=await browser.newPage();let sourceBinding;
  const observed=[];
  try {
    const response=await probe.goto(base);assert.equal(response.status(),200);
    assert.ok((await response.body()).equals(await fs.readFile(path.join(root,'console/index.html'))));
    sourceBinding={expected:response.headers().etag,responseHeader:'etag'};
    const config=agentHubFormalConfig({...options,theme:'light',cookie:'unused',sourceBinding});
    for(const file of config.targets[0].reviewInputs){
      const served=await probe.request.get(new URL(file.path.slice('console/'.length),base).href);
      const bytes=await served.body();assert.equal(served.status(),200);assert.ok(bytes.equals(await fs.readFile(path.join(root,file.path))),file.path);
      observed.push({path:file.path,sha256:createHash('sha256').update(bytes).digest('hex')});
    }
    await fs.writeFile(path.join(out,`agent-hub-${options.firstRun?'first-run-':''}served-inputs.json`),JSON.stringify({sourceBinding,files:observed},null,2));
  } finally {await probe.close();}
  const results=[];
  for(const role of options.firstRun?['administrator']:['administrator','viewer'])for(const theme of ['light','dark']) {
    const suffix=`${options.firstRun?'first-run-':''}${role==='viewer'?'viewer-':''}${theme}`;
    const directory=path.join(out,`formal-${suffix}`);await fs.mkdir(directory);
    const config=agentHubFormalConfig({...options,role,theme,sourceBinding,cookie:`dc2_session=${cookieFor(role==='viewer'?'viewer@example.test':'owner@example.test').value}`,playwrightModuleDir:process.env.FORMAL_WEB_UI_PLAYWRIGHT_NODE_MODULES||'/home/DevCoordinator2/ci/playwright/node_modules'});
    const input=path.join(temporary,`agent-hub-formal-${suffix}.json`);await fs.writeFile(input,JSON.stringify(config),{mode:0o600});
    const args=[path.join(root,'skills/formal-web-ui-verification/scripts/formal_web_ui_verify.mjs'),'--config',input,'--json-out',path.join(directory,'report.json'),'--markdown-out',path.join(directory,'report.md'),'--screenshot-dir',path.join(directory,'screenshots')];
    let stdout='',exitCode=0;
    try {({stdout}=await promisify(execFile)(process.execPath,args,{cwd:root,maxBuffer:1024*1024}));}
    catch(error){stdout=error.stdout||'';exitCode=error.code;await fs.writeFile(path.join(directory,'runner-error.log'),error.stderr||`Formal verifier exited ${exitCode}`);}
    await fs.writeFile(path.join(directory,'receipt.json'),stdout);
    const receipt=JSON.parse(stdout);results.push({role,theme,formal:receipt.formal,receipt:path.join(directory,'receipt.json'),queue:path.join(directory,'review-queue.json')});
  }
  await fs.writeFile(path.join(out,`agent-hub-formal-${options.firstRun?'first-run-':''}results.json`),JSON.stringify(results,null,2));
  assert.ok(results.every(row=>row.formal?.result==='passed'),'Every light/dark formal candidate must have formal.result=passed; inspect agent-hub-formal-results.json');
}
