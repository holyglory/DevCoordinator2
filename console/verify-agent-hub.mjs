// Acceptance extension of verify-glossary.mjs: real edge, typed dispatch and
// disposable PostgreSQL authority. Credentials exist in memory only.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { verifyAgentHubFormal } from './agent-hub-formal.mjs';

const action = name => `[data-hub-action="${name}"]`;
const auth = registration => ({agent_id:registration.agent.agent_id,token:registration.token});
export function fixturePdf() {
  const objects=['<< /Type /Catalog /Pages 2 0 R >>','<< /Type /Pages /Kids [3 0 R] /Count 1 >>','<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>','<< /Length 25 >>\nstream\n0.2 g 20 20 160 160 re f\nendstream'];
  let value='%PDF-1.4\n';const offsets=[0];
  for(const [index,body]of objects.entries()){offsets.push(Buffer.byteLength(value));value+=`${index+1} 0 obj\n${body}\nendobj\n`;}
  const xref=Buffer.byteLength(value);value+=`xref\n0 5\n0000000000 65535 f \n${offsets.slice(1).map(offset=>`${String(offset).padStart(10,'0')} 00000 n \n`).join('')}trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(value);
}

export async function verifyAgentHub({page,browser,base,call,request,cookieFor,out,root,temporary,setOffline}) {
  const checks=[], secrets=[];
  const retainSecret=value=>{if(value)secrets.push(value);return value;};
  const redact=value=>secrets.reduce((text,secret)=>text.replaceAll(secret,'[REDACTED]'),String(value));
  const masks=()=>[page.locator('[data-hub-secret]'),page.locator('#who-email'),page.locator('#workspace-root')];
  const check=async(name,run)=>{
    const start=Date.now();
    try {await run();checks.push({name,status:'passed',duration_ms:Date.now()-start});}
    catch(error){
      const filename=`hub-failure-${checks.length}.log`;
      await fs.writeFile(path.join(out,filename),redact(error.stack||error));
      // Never retain DOM dumps, traces, request/response bodies or entered keys.
      await page.screenshot({path:path.join(out,`${filename}.png`),fullPage:true,mask:masks()}).catch(()=>{});
      checks.push({name,status:'failed',duration_ms:Date.now()-start,evidence:filename});
      // A failed modal case must not block independent later assertions.
      const close=page.locator('dialog[open] [data-hub-action="close-preview"]');
      if(await close.count())await close.click().catch(()=>{});
      if(await page.locator('.hub-key-dialog[open]').count())await page.keyboard.press('Escape').catch(()=>{});
      if(await page.locator('.hub-commands-dialog[open]').count())await page.keyboard.press('Escape').catch(()=>{});
    }
    await fs.writeFile(path.join(out,'agent-hub-progress.json'),JSON.stringify({completed:checks.length,last:checks.at(-1)},null,2));
  };
  const go=async(step='key',query='')=>{
    await page.goto(`${base}#/agent-hub?step=${step}${query}`);
    await page.locator(`.hub-step[data-hub-step="${step}"][aria-current="step"]`).waitFor();
    await page.locator('.hub-panel[aria-busy="false"]').waitFor();
  };
  const remote=async(action,bearer)=>{
    const response=await fetch(new URL('/.well-known/devcoordinator2/agent-hub',base),{method:'POST',headers:{'content-type':'application/json',...(bearer?{authorization:`Bearer ${bearer}`}:{})},body:JSON.stringify({action})});
    return response.json();
  };
  const register=async(name,space='private')=>{
    const result=await call('agent.hub.register',{space,display_name:name,repository_label:'Acceptance fixture',current_subject:'Verify a handoff',capabilities:['browser-checks']},null);
    retainSecret(result.token);retainSecret(result.callback_secret);
    await call('fixture.hub.lease',{agent_id:result.agent.agent_id},null);
    return result;
  };
  const contents=async()=>page.locator('main').innerText();
  const fullyVisible=async selector=>{
    const result=await page.locator(selector).first().evaluate(element=>{
      const rect=element.getBoundingClientRect();const dialog=element.closest('dialog');const limit=dialog?.getBoundingClientRect();
      return {visible:rect.width>0&&rect.height>0&&rect.left>=0&&rect.top>=0&&rect.right<=innerWidth&&rect.bottom<=innerHeight&&(!limit||(rect.top>=limit.top&&rect.bottom<=limit.bottom))};
    });
    assert.ok(result.visible,`Fully visible continuation: ${selector}`);
  };
  const selectRoom=async id=>{
    const select=page.locator('#hub-room-select');
    if(await select.isVisible())await select.selectOption(id);else await page.locator(`[data-hub-room="${id}"]`).click();
    await page.locator('.hub-panel[aria-busy="false"]').waitFor();
  };
  const privateKeyCapture=async(key,label)=>{
    const guard=await page.evaluate(secret=>{
      const elements=[...document.querySelectorAll('*')];if(elements.length>4000)throw new Error('Key guard DOM bound exceeded');
      const field=document.querySelector('[data-hub-key-value]');
      return {readonly:field?.readOnly===true,valueMatches:field?.value===secret,
        otherValues:[...document.querySelectorAll('input,textarea,select')].filter(element=>element!==field&&String(element.value).includes(secret)).length,
        attributeLeaks:elements.reduce((count,element)=>count+[...element.attributes].filter(attribute=>attribute.value.includes(secret)).length,0),
        visibleTextLeak:document.body.innerText.includes(secret),
        storedLeak:JSON.stringify({local:{...localStorage},session:{...sessionStorage},history:history.state,cookies:document.cookie,url:location.href}).includes(secret)};
    },key);
    assert.ok(guard.readonly&&guard.valueMatches&&guard.otherValues===0&&guard.attributeLeaks===0&&!guard.visibleTextLeak&&!guard.storedLeak,'The one-time key exists only in its readonly field');
    const selectors=['#hub-key-dialog-title','#hub-key-dialog-description','[data-hub-action="copy-key"]','[data-hub-action="dismiss-key"]'];
    for(const selector of selectors)await fullyVisible(selector);
    const regions=await Promise.all(['[data-hub-key-value]',...selectors].map(async selector=>({selector,rect:await page.locator(selector).boundingBox()})));
    const bytes=await page.screenshot({mask:[page.locator('[data-hub-key-value]'),page.locator('#who-email'),page.locator('#workspace-root')],maskColor:'#777777'});
    const pixels=await page.evaluate(async({png,regions})=>{
      const image=new Image();image.src=`data:image/png;base64,${png}`;await image.decode();const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
      const context=canvas.getContext('2d');context.drawImage(image,0,0);const scale=image.width/innerWidth;
      return regions.map(({rect})=>{const data=context.getImageData(Math.floor((rect.x+rect.width*.25)*scale),Math.floor((rect.y+rect.height*.25)*scale),Math.max(1,Math.floor(rect.width*.5*scale)),Math.max(1,Math.floor(rect.height*.5*scale))).data;let masked=0;for(let i=0;i<data.length;i+=4)if(data[i]===119&&data[i+1]===119&&data[i+2]===119)masked++;return {samples:data.length/4,masked};});
    },{png:bytes.toString('base64'),regions});
    assert.equal(pixels[0].masked,pixels[0].samples,'Screenshot masks the entire key field');
    assert.ok(pixels.slice(1).every(region=>region.masked<region.samples),'Screenshot retains title, instructions and both key actions');
    await fs.writeFile(path.join(out,`key-${label}-redacted.png`),bytes);
    await fs.writeFile(path.join(out,`key-${label}-guard.json`),JSON.stringify({guard,fieldRedacted:true,visibleControls:selectors.length},null,2));
  };
  page.setDefaultTimeout(10000);
  page.on('dialog',dialog=>dialog.accept());
  let firstKey,secondKey,sender,recipient,expired,publicAgent,room,emptyRoom,publicRoom,zeroRoom,fileMessage,pendingMessage,ackedMessage,zeroMessage;
  await check('The normal Console menu reaches Agent hub',async()=>{
    await page.goto(`${base}#/health`);
    await page.locator('#nav-toggle:enabled').click();
    await page.locator('#nav-toggle[aria-expanded="true"]').waitFor();
    await page.locator('#nav a[data-view="agent-hub"]').click();
    await page.locator('.hub-step[data-hub-step="key"][aria-current="step"]').waitFor();
    assert.match(page.url(),/#\/agent-hub/);
  });
  await check('An unused hub lists an honest empty room collection',async()=>{
    const listed=await call('agent.hub.console.rooms',{space:'private',limit:20});
    assert.deepEqual(listed.rooms,[]);
    await go('rooms');
    assert.match(await contents(),/no rooms|rooms.*appear|empty/i);
    assert.equal(await page.locator('[data-hub-room]').count(),0);
  });
  if(process.env.AGENT_HUB_FORMAL==='1')await check('Fresh formal first-run Agent hub before any key exists',async()=>{
    assert.equal((await call('agent.hub.settings.get')).current_key_created_at,null);
    await verifyAgentHubFormal({base,root,out,temporary,browser,cookieFor,firstRun:true});
  });
  await check('The local connection path explains socket access without requiring a key',async()=>{
    await go('connect');
    await page.locator('#hub-client').selectOption('local');
    assert.match(await contents(),/local|socket/i);
    assert.equal(await page.locator('[data-hub-secret]').count(),0);
    await page.locator('#hub-client').selectOption('remote');
    assert.match(await contents(),/Bearer|instance key|private key/i);
    assert.equal(await page.locator('[data-hub-secret]').count(),0);
  });
  await check('Copyable onboarding uses valid local syntax and distinguishes instance and agent credentials',async()=>{
    await page.context().grantPermissions(['clipboard-read','clipboard-write']);
    await go('connect');
    for(const client of ['local','remote','public']) {
      await page.locator('#hub-client').selectOption(client);
      await page.locator(action('copy-instructions')).click();
      await page.locator('.hub-notice').filter({hasText:/copied/i}).waitFor();
      const text=await page.evaluate(()=>navigator.clipboard.readText());
      assert.ok(!secrets.some(secret=>text.includes(secret)));
      if(client==='local') {
        assert.match(text,/devcoordinator2 call agent\.hub\.register --params /);
        assert.match(text,/agent\.hub\.send --params /);
        for(const command of text.split('\n').filter(line=>line.startsWith('devcoordinator2 call '))) {
          const match=command.match(/^devcoordinator2 call (agent\.hub\.[\w.]+) --params '(.*)'$/);
          assert.ok(match,'Every local example uses the real CLI syntax');JSON.parse(match[2]);
        }
      } else if(client==='remote') {
        assert.match(text,/Bearer/);assert.match(text,/instance key/i);assert.match(text,/registration.token|agent.token|token.*register/i);
      }
      assert.match(text,/"members"\s*:\s*\[\s*"[^"\]]+"/);
      assert.match(text,/"kind"\s*:\s*"send"|agent\.hub\.send/);
      assert.match(text,/receive/);assert.match(text,/ack/);
    }
    await page.evaluate(()=>navigator.clipboard.writeText(''));
  });
  await check('Generate, copy and dismiss reveal a real instance key exactly once',async()=>{
    await page.setViewportSize({width:390,height:664});
    try {
    await go();
    const settings=await call('agent.hub.settings.get');
    assert.equal(settings.current_key_created_at,null);
    await page.context().grantPermissions(['clipboard-read','clipboard-write']);
    await page.locator(action('generate-key')).scrollIntoViewIfNeeded();
    const scroll=await page.evaluate(()=>scrollY);
    await page.locator(action('generate-key')).click();
    await page.locator('[data-hub-key-value]').waitFor();
    firstKey=retainSecret(await page.locator('[data-hub-key-value]').evaluate(el=>el.value||el.textContent));
    assert.ok(await page.evaluate(position=>Math.abs(scrollY-position)<=8,scroll),'Initial phone reveal preserves document position');
    assert.equal(await page.evaluate(()=>document.activeElement?.id),'hub-key');
    await privateKeyCapture(firstKey,'initial-phone');
    assert.ok(firstKey.length>=32,'A key was generated');
    await page.locator(action('copy-key')).click();
    await page.locator('.hub-notice').filter({hasText:/copied/i}).waitFor();
    assert.ok(await page.evaluate(async expected=>(await navigator.clipboard.readText())===expected,firstKey),'Copy places the revealed key on the clipboard');
    await page.evaluate(()=>navigator.clipboard.writeText(''));
    const accepted=await remote({kind:'directory',space:'private'},firstKey);
    assert.equal(accepted.ok,true);
    await page.locator(action('dismiss-key')).click();
    assert.equal(await page.locator('[data-hub-key-value]').count(),0);
    assert.equal(await page.evaluate(()=>document.activeElement?.dataset.hubAction),'generate-key');
    await go('key');
    assert.equal(await page.locator('[data-hub-key-value]').count(),0);
    assert.ok(!(await contents()).includes(firstKey));
    const stored=JSON.stringify(await call('agent.hub.settings.get'));
    assert.ok(!stored.includes(firstKey));
    const browserStorage=await page.evaluate(()=>JSON.stringify({local:{...localStorage},session:{...sessionStorage}}));
    assert.ok(!browserStorage.includes(firstKey),'The one-time key is absent from browser storage');
    } finally {await page.setViewportSize({width:1440,height:900});}
  });
  await check('Phone rotation keeps keys private, preserves the overlap and returns focus without a page jump',async()=>{
    await page.setViewportSize({width:390,height:664});
    try {
    await go('key');
    const before=await call('agent.hub.settings.get');
    await page.locator(action('generate-key')).scrollIntoViewIfNeeded();
    const scroll=await page.evaluate(()=>scrollY);
    await page.locator(action('generate-key')).click();
    await page.locator('.hub-key-dialog[open] .hub-confirm').waitFor();
    assert.ok(await page.evaluate(position=>Math.abs(scrollY-position)<=8,scroll),'Confirmation remains at the current document position');
    await page.locator(action('cancel-rotate')).click();
    assert.equal((await call('agent.hub.settings.get')).revision,before.revision);
    await page.locator(action('generate-key')).click();
    await page.locator(action('confirm-rotate')).click();
    await page.locator('[data-hub-key-value]').waitFor();
    assert.ok(await page.evaluate(position=>Math.abs(scrollY-position)<=8,scroll),'Reveal remains at the current document position');
    secondKey=retainSecret(await page.locator('[data-hub-key-value]').evaluate(el=>el.value||el.textContent));
    await privateKeyCapture(secondKey,'rotated-phone');
    assert.ok(secondKey!==firstKey,'Rotation generates another key');
    assert.equal((await remote({kind:'directory',space:'private'},secondKey)).ok,true);
    assert.equal((await remote({kind:'directory',space:'private'},firstKey)).ok,true);
    const settings=await call('agent.hub.settings.get');
    assert.ok(Date.parse(settings.previous_key_expires_at)>Date.now()+6*86400000);
    await go('connect');
    await go('key');
    assert.equal(await page.locator('[data-hub-key-value]').count(),0);
    assert.equal((await remote({kind:'directory',space:'private'},secondKey)).ok,true);
    await page.locator(action('generate-key')).click();await page.locator(action('confirm-rotate')).click();await page.locator('[data-hub-key-value]').waitFor();
    const escapedKey=retainSecret(await page.locator('[data-hub-key-value]').evaluate(el=>el.value||el.textContent));
    await page.keyboard.press('Escape');
    await page.locator('.hub-key-dialog[open]').waitFor({state:'detached'});
    assert.equal(await page.locator('[data-hub-key-value]').count(),0);
    assert.equal(await page.evaluate(()=>document.activeElement?.dataset.hubAction),'generate-key');
    assert.equal((await remote({kind:'directory',space:'private'},escapedKey)).ok,true);
    } finally {await page.setViewportSize({width:1440,height:900});}
  });
  await check('Viewer access cannot create keys, change public settings or moderate rooms',async()=>{
    for(const [operation,params] of [
      ['agent.hub.key.rotate',{expected_revision:0}],
      ['agent.hub.settings.set',{expected_revision:0,public_enabled:true,public_paused:false}],
      ['agent.hub.admin.room.remove',{room_id:'h0000000000000000'}],
    ]){const result=await request(operation,params,'viewer@example.test');assert.equal(result.ok,false);assert.equal(result.error.code,'permission_denied');}
    const viewer=await browser.newContext({viewport:{width:390,height:844}});
    try {
      await viewer.addCookies([cookieFor('viewer@example.test')]);const tab=await viewer.newPage();
      await tab.goto(`${base}#/agent-hub?step=key`);await tab.locator('[data-hub-step="key"]').waitFor();
      assert.equal(await tab.locator(`${action('generate-key')}:not([disabled])`).count(),0);
      await tab.locator(action('public-settings')).click();
      assert.equal(await tab.locator(`${action('enable')}:not([disabled])`).count(),0);
      assert.equal(await tab.locator('[data-hub-secret]').count(),0);
    } finally {await viewer.close();}
  });
  await check('Public registration is unavailable until the administrator enables it',async()=>{
    const blocked=await remote({kind:'register',space:'public',display_name:'Public boundary probe'});
    assert.equal(blocked.ok,false);assert.equal(blocked.error.code,'hub_disabled');
    await go('key');
    await page.locator(action('public-settings')).click();
    await page.locator(action('enable')).click();
    await page.locator(`${action('pause')}:not([disabled])`).waitFor();
    assert.equal((await call('agent.hub.settings.get')).public_enabled,true);
  });
  await check('Real agents and rooms prepare isolated private and public journeys',async()=>{
    sender=await register('Build agent');recipient=await register('Review agent');expired=await register('Expired agent');
    await call('fixture.hub.lease',{agent_id:expired.agent.agent_id,expired:true},null);
    const registered=await remote({kind:'register',space:'public',display_name:'Public collaborator',current_subject:'Review public work'});
    assert.equal(registered.ok,true);publicAgent=registered.data;retainSecret(publicAgent.token);
    await call('fixture.hub.lease',{agent_id:publicAgent.agent.agent_id},null);
    room=await call('agent.hub.room.create',{auth:auth(sender),space:'private',name:'Release handoff',members:[recipient.agent.agent_id]},null);
    emptyRoom=await call('agent.hub.room.create',{auth:auth(sender),space:'private',name:'Empty review room',members:[]},null);
    zeroRoom=await call('agent.hub.room.create',{auth:auth(sender),space:'private',name:'No recipients',members:[]},null);
    const created=await remote({kind:'room_create',auth:auth(publicAgent),space:'public',name:'Public collaboration',members:[]});
    assert.equal(created.ok,true);publicRoom=created.data;
    const text=Buffer.from('This attachment is safe fixture text.\n<script>window.hubUnsafe=true</script>');
    const upload=await call('agent.hub.upload.start',{auth:auth(sender),request_key:'browser-fixture-file',name:'handoff.txt',content_type:'text/plain',byte_size:text.length,sha256:createHash('sha256').update(text).digest('hex')},null);
    await call('agent.hub.upload.chunk',{auth:auth(sender),upload_id:upload.upload_id,offset:0,data_base64:text.toString('base64')},null);
    const pdf=fixturePdf();
    const pdfUpload=await call('agent.hub.upload.start',{auth:auth(sender),request_key:'browser-fixture-pdf',name:'handoff.pdf',content_type:'application/pdf',byte_size:pdf.length,sha256:createHash('sha256').update(pdf).digest('hex')},null);
    await call('agent.hub.upload.chunk',{auth:auth(sender),upload_id:pdfUpload.upload_id,offset:0,data_base64:pdf.toString('base64')},null);
    const png=Buffer.from(await page.evaluate(()=>{
      const canvas=document.createElement('canvas');canvas.width=640;canvas.height=360;const context=canvas.getContext('2d');
      context.fillStyle='#e8f2ff';context.fillRect(0,0,640,360);context.fillStyle='#234c79';context.fillRect(48,64,180,180);context.fillRect(412,64,180,180);context.fillRect(228,148,184,12);
      context.font='24px sans-serif';context.fillText('Safe fixture diagram',48,310);return canvas.toDataURL('image/png').split(',')[1];
    }),'base64');
    const imageUpload=await call('agent.hub.upload.start',{auth:auth(sender),request_key:'browser-fixture-png',name:'handoff-diagram.png',content_type:'image/png',byte_size:png.length,sha256:createHash('sha256').update(png).digest('hex')},null);
    await call('agent.hub.upload.chunk',{auth:auth(sender),upload_id:imageUpload.upload_id,offset:0,data_base64:png.toString('base64')},null);
    fileMessage=await call('agent.hub.send',{auth:auth(sender),room_id:room.room_id,body:'Please inspect the attached handoff notes.',attachments:[upload.upload_id,pdfUpload.upload_id,imageUpload.upload_id]},null);
    ackedMessage=await call('agent.hub.send',{auth:auth(sender),room_id:room.room_id,body:'Acknowledged handoff fixture.',attachments:[]},null);
    const deliveries=await call('agent.hub.receive',{auth:auth(recipient),room_id:room.room_id},null);
    await call('agent.hub.ack',{auth:auth(recipient),delivery_ids:[deliveries.deliveries.find(row=>row.message.message_id===ackedMessage.message_id).delivery_id]},null);
    pendingMessage=await call('agent.hub.send',{auth:auth(sender),room_id:room.room_id,body:'Pending handoff fixture.',attachments:[]},null);
    zeroMessage=await call('agent.hub.send',{auth:auth(sender),room_id:zeroRoom.room_id,body:'Stored without recipient fixture.',attachments:[]},null);
    const sent=await remote({kind:'send',auth:auth(publicAgent),room_id:publicRoom.room_id,body:'Public message fixture.',attachments:[]});assert.equal(sent.ok,true);
  });
  await check('The directory exposes active and expired presence without private transport secrets',async()=>{
    await go('rooms');
    const directory=await call('agent.hub.console.directory',{space:'private',limit:50});
    assert.equal(directory.agents.find(row=>row.agent_id===expired.agent.agent_id).state,'expired');
    assert.equal(directory.agents.find(row=>row.agent_id===sender.agent.agent_id).state,'active');
    assert.ok(!secrets.some(secret=>JSON.stringify(directory).includes(secret)));
    assert.match(await contents(),/Build agent/);assert.match(await contents(),/Expired agent/);assert.match(await contents(),/Expired/i);
    assert.equal(await page.locator('[data-hub-room]').count(),3);
  });
  await check('Selecting an actual empty room keeps its no-message result truthful',async()=>{
    await selectRoom(emptyRoom.room_id);
    await page.locator('.hub-conversation .hub-empty').filter({hasText:/no messages/i}).waitFor();
    assert.match(await contents(),/no messages|empty|no deliveries/i);
    assert.equal(await page.locator('[data-hub-delivery]').count(),0);
  });
  await check('An admitted viewer reads and replies while administrator controls stay unavailable',async()=>{
    const context=await browser.newContext({viewport:{width:1487,height:1058}});
    try {
      await context.addCookies([cookieFor('viewer@example.test')]);const tab=await context.newPage();
      await tab.goto(`${base}#/agent-hub?step=delivery&room=${room.room_id}`);
      await tab.locator(`[data-hub-delivery="${fileMessage.message_id}"]`).waitFor();
      assert.equal(await tab.locator(`${action('send')}:not([disabled])`).count(),1);
      assert.equal(await tab.locator('[data-hub-action^="remove-"]:not([disabled])').count(),0);
      assert.equal(await tab.locator('.hub-error').count(),0);
      const read=await request('agent.hub.console.receive',{space:'private',room_id:room.room_id},'viewer@example.test');assert.equal(read.ok,true);
      await tab.locator('#hub-reply').fill('Viewer reply from the isolated browser fixture.');await tab.locator(action('send')).click();
      await tab.getByText('Viewer reply from the isolated browser fixture.',{exact:true}).waitFor();
      assert.ok((await call('agent.hub.console.receive',{space:'private',room_id:room.room_id})).deliveries.some(row=>row.message.body==='Viewer reply from the isolated browser fixture.'));
    } finally {await context.close();}
  });
  await check('Private and public Console collections and remote discovery remain isolated',async()=>{
    await page.locator('#hub-space').selectOption('public');
    await page.locator(`[data-hub-room="${publicRoom.room_id}"]`).waitFor();
    assert.equal(await page.locator(`[data-hub-room="${room.room_id}"]`).count(),0);
    assert.ok(!(await contents()).includes('Build agent'));
    const publicDirectory=await remote({kind:'directory',space:'public'});assert.equal(publicDirectory.ok,true);
    assert.ok(publicDirectory.data.agents.every(row=>row.space==='public'));
    const privateDirectory=await remote({kind:'directory',space:'private'});assert.equal(privateDirectory.ok,false);
    for(const [operation,params] of [
      ['agent.hub.console.receive',{space:'public',room_id:room.room_id}],
      ['agent.hub.console.file',{space:'public',room_id:room.room_id,file_id:fileMessage.attachments[0].attachment_id}],
      ['agent.hub.console.send',{space:'public',room_id:room.room_id,body:'Wrong space'}],
    ])assert.equal((await request(operation,params)).ok,false);
  });
  await check('Delivery status reflects persisted recipient receipts including zero recipients',async()=>{
    const delivered=await call('agent.hub.console.receive',{space:'private',room_id:room.room_id});
    assert.deepEqual(delivered.deliveries.find(row=>row.message.message_id===ackedMessage.message_id).receipt_counts,{recipients:1,acknowledged:1});
    assert.deepEqual(delivered.deliveries.find(row=>row.message.message_id===pendingMessage.message_id).receipt_counts,{recipients:1,acknowledged:0});
    const noRecipients=await call('agent.hub.console.receive',{space:'private',room_id:zeroRoom.room_id});
    assert.deepEqual(noRecipients.deliveries[0].receipt_counts,{recipients:0,acknowledged:0});assert.equal(noRecipients.deliveries[0].acknowledged,false);
    await go('delivery',`&room=${room.room_id}`);
    await page.locator(`[data-hub-delivery="${ackedMessage.message_id}"]`).waitFor();
    assert.match(await page.locator(`[data-hub-delivery="${ackedMessage.message_id}"]`).innerText(),/acknowledged|confirmed/i);
    assert.match(await page.locator(`[data-hub-delivery="${pendingMessage.message_id}"] [data-hub-receipt]`).innerText(),/Acknowledged:\s*0\s*\/\s*1/i);
    await go('delivery',`&room=${zeroRoom.room_id}`);
    assert.match(await contents(),/no recipients|stored.*only|stored without/i);
  });
  await check('Each command reference opens a fully visible first command and restores focus',async()=>{
    for(const width of [390,1487]) {
      await page.setViewportSize({width,height:width===390?664:1058});
      for(const step of ['connect','rooms','delivery']) {
        await go(step,step==='delivery'?`&room=${room.room_id}`:'');
        await page.locator(action('open-commands')).scrollIntoViewIfNeeded();const scroll=await page.evaluate(()=>scrollY);
        await page.locator(action('open-commands')).click();await page.locator('.hub-commands-dialog[open]').waitFor();
        await fullyVisible('#hub-commands-title');await fullyVisible('.hub-commands-dialog .hub-instruction:first-child pre');
        assert.ok(await page.evaluate(position=>Math.abs(scrollY-position)<=8,scroll),'Command dialog preserves document position');
        assert.ok(await page.locator('.hub-commands-dialog').evaluate(element=>element.contains(document.activeElement)));
        if(step==='connect')await page.keyboard.press('Escape');else await page.locator(action('close-commands')).click();
        assert.equal(await page.evaluate(()=>document.activeElement?.id),`hub-open-commands-${step}`);
      }
    }
    await page.setViewportSize({width:1440,height:900});await go('delivery',`&room=${room.room_id}`);
  });
  await check('Phone monitoring exposes actual room selection, receipts and empty results above the fold',async()=>{
    await page.setViewportSize({width:390,height:664});
    try {
      await go('rooms');await fullyVisible('[data-hub-room]');
      for(const selected of [room,zeroRoom,publicRoom]) {
        await go('delivery',`&room=${selected.room_id}${selected===publicRoom?'&space=public':''}`);
        await fullyVisible('.hub-room-heading h3');await fullyVisible('[data-hub-receipt]');
        assert.equal(await page.locator('#hub-room-select').inputValue(),selected.room_id);
      }
      await go('delivery',`&room=${emptyRoom.room_id}`);await fullyVisible('.hub-conversation .hub-empty');
      await selectRoom(room.room_id);await fullyVisible('[data-hub-receipt]');
    } finally {await page.setViewportSize({width:1440,height:900});await go('delivery',`&room=${room.room_id}`);}
  });
  await check('Attachment preview treats text as text and download returns verified bytes',async()=>{
    await go('delivery',`&room=${room.room_id}`);
    const row=page.locator(`[data-hub-delivery="${fileMessage.message_id}"]`);await row.waitFor();
    await row.locator(action('preview')).first().click();
    await page.locator('.hub-preview pre').waitFor();
    assert.match(await page.locator('.hub-preview pre').innerText(),/<script>/);
    assert.equal(await page.evaluate(()=>window.hubUnsafe),undefined);
    await page.locator(action('close-preview')).click();
    const downloadPromise=page.waitForEvent('download');await row.locator(action('download')).first().click();const download=await downloadPromise;
    assert.equal(download.suggestedFilename(),'handoff.txt');
    const bytes=await fs.readFile(await download.path());
    assert.equal(createHash('sha256').update(bytes).digest('hex'),fileMessage.attachments[0].sha256);
    await download.delete();
  });
  await check('PDF preview loads its real module and worker and renders verified attachment bytes',async()=>{
    const pdfModule=await page.request.get(new URL('/vendor/pdfjs/pdf.min.mjs',base).href);assert.equal(pdfModule.status(),200);assert.match(pdfModule.headers()['content-type'],/javascript/);
    const worker=await page.request.get(new URL('/vendor/pdfjs/pdf.worker.min.mjs',base).href);assert.equal(worker.status(),200);assert.match(worker.headers()['content-type'],/javascript/);
    await page.locator(`[data-hub-delivery="${fileMessage.message_id}"]`).locator(action('preview')).nth(1).click();
    try {await page.locator('.hub-preview canvas').waitFor();}
    catch(error) {
      const diagnostic=await page.evaluate(async bytes=>{
        try {
          const pdf=await import('/vendor/pdfjs/pdf.min.mjs');pdf.GlobalWorkerOptions.workerSrc='/vendor/pdfjs/pdf.worker.min.mjs';
          const task=pdf.getDocument({data:new Uint8Array(bytes),isEvalSupported:false,enableXfa:false}),document=await task.promise;
          const page=await document.getPage(1),viewport=page.getViewport({scale:1}),canvas=window.document.createElement('canvas');canvas.width=viewport.width;canvas.height=viewport.height;
          await page.render({canvasContext:canvas.getContext('2d'),viewport}).promise;await task.destroy();
          return {independentFixtureRender:'passed',width:canvas.width,height:canvas.height};
        } catch(error) {return {independentFixtureRender:'failed',name:error.name,message:error.message};}
      },[...fixturePdf()]);
      await fs.writeFile(path.join(out,'pdf-preview-diagnostic.json'),JSON.stringify(diagnostic,null,2));throw error;
    }
    assert.ok(await page.locator('.hub-preview canvas').evaluate(canvas=>canvas.width>0&&canvas.height>0));
    await page.locator(action('close-preview')).click();
  });
  await check('PNG preview decodes the real attachment and download preserves its SHA-256',async()=>{
    const row=page.locator(`[data-hub-delivery="${fileMessage.message_id}"]`);
    await row.locator(action('preview')).nth(2).click();
    await page.waitForFunction(()=>{const image=document.querySelector('.hub-preview img');return image?.complete&&image.naturalWidth===640&&image.naturalHeight===360;});
    await page.locator(action('close-preview')).click();
    const downloadPromise=page.waitForEvent('download');await row.locator(action('download')).nth(2).click();const download=await downloadPromise;
    assert.equal(download.suggestedFilename(),'handoff-diagram.png');
    const bytes=await fs.readFile(await download.path());assert.equal(createHash('sha256').update(bytes).digest('hex'),fileMessage.attachments[2].sha256);await download.delete();
  });
  await check('Human reply reaches the real room, survives refresh and is acknowledged by a recipient',async()=>{
    await page.locator('#hub-reply').fill('Human approval from the browser fixture.');await page.locator(action('send')).click();
    await page.getByText('Human approval from the browser fixture.',{exact:true}).waitFor();
    let stored=await call('agent.hub.console.receive',{space:'private',room_id:room.room_id});
    const human=stored.deliveries.find(row=>row.message.body==='Human approval from the browser fixture.');assert.ok(human);
    await page.reload();await page.getByText('Human approval from the browser fixture.',{exact:true}).waitFor();
    const inbox=await call('agent.hub.receive',{auth:auth(recipient),room_id:room.room_id},null);
    const delivery=inbox.deliveries.find(row=>row.message.message_id===human.message.message_id);assert.ok(delivery);
    await call('agent.hub.ack',{auth:auth(recipient),delivery_ids:[delivery.delivery_id]},null);
    await page.locator(action('refresh')).click();
    stored=await call('agent.hub.console.receive',{space:'private',room_id:room.room_id});assert.equal(stored.deliveries.find(row=>row.message.message_id===human.message.message_id).receipt_counts.acknowledged,1);
  });
  await check('Collection continuation keeps already displayed messages without duplicating them',async()=>{
    const before=await page.locator('[data-hub-delivery]').first().getAttribute('data-hub-delivery').catch(()=>null);
    const more=page.locator(action('load-messages'));
    if(await more.count()){await more.click();await page.locator('.hub-panel[aria-busy="false"]').waitFor();}
    const ids=await page.locator('[data-hub-delivery]').evaluateAll(rows=>rows.map(row=>row.dataset.hubDelivery));
    assert.equal(new Set(ids).size,ids.length);assert.ok(ids.includes(fileMessage.message_id));
    if(before)assert.ok(ids.includes(before));
  });
  await check('Administrator moderation removes only the selected fixture message, room and agent',async()=>{
    const disposable=await register('Disposable moderation agent');
    const extra=await call('agent.hub.room.create',{auth:auth(sender),space:'private',name:'Disposable moderation room',members:[]},null);
    const message=await call('agent.hub.send',{auth:auth(sender),room_id:extra.room_id,body:'Disposable moderation message.',attachments:[]},null);
    await go('delivery',`&room=${extra.room_id}`);
    await page.locator(`[data-hub-delivery="${message.message_id}"]`).locator(action('remove-message')).click();
    await page.locator(`[data-hub-delivery="${message.message_id}"]`).waitFor({state:'detached'});
    assert.equal((await call('agent.hub.console.receive',{space:'private',room_id:extra.room_id})).deliveries.length,0);
    await page.locator(action('remove-room')).click();
    await page.locator(`[data-hub-room="${extra.room_id}"]`).waitFor({state:'detached'});
    await page.locator(`[data-hub-agent="${disposable.agent.agent_id}"]`).locator(action('remove-agent')).click();
    await page.locator(`[data-hub-agent="${disposable.agent.agent_id}"]`).waitFor({state:'detached'});
    assert.ok((await call('agent.hub.console.rooms',{space:'private'})).rooms.some(row=>row.room_id===room.room_id));
    await go('delivery',`&room=${room.room_id}`);
  });
  await check('Pausing public access blocks new remote work while retained Console history remains readable',async()=>{
    await go('key');await page.locator(action('public-settings')).click();await page.locator(action('pause')).click();
    await page.locator(`${action('resume')}:not([disabled])`).waitFor();
    assert.equal((await call('agent.hub.settings.get')).public_paused,true);
    assert.equal((await remote({kind:'directory',space:'public'})).ok,false);
    assert.ok((await call('agent.hub.console.rooms',{space:'public'})).rooms.some(row=>row.room_id===publicRoom.room_id));
    for(const identity of ['owner@example.test','viewer@example.test']) {
      const context=await browser.newContext();
      try {
        await context.addCookies([cookieFor(identity)]);const tab=await context.newPage();
        await tab.goto(`${base}#/agent-hub?step=delivery&space=public&room=${publicRoom.room_id}`);await tab.getByText('Public message fixture.',{exact:true}).waitFor();
        assert.equal(await tab.locator(`${action('send')}:not([disabled])`).count(),0);
        const denied=await request('agent.hub.console.send',{space:'public',room_id:publicRoom.room_id,body:'Paused public reply'},identity);assert.equal(denied.ok,false);assert.equal(denied.error.code,'hub_disabled');
      } finally {await context.close();}
    }
    await page.locator(action('resume')).click();
    await page.locator(`${action('pause')}:not([disabled])`).waitFor();
    await page.locator(action('disable')).click();
    await page.locator(`${action('enable')}:not([disabled])`).waitFor();
    assert.equal((await call('agent.hub.settings.get')).public_enabled,false);
    assert.equal((await remote({kind:'directory',space:'public'})).ok,false);
    assert.ok((await call('agent.hub.console.receive',{space:'public',room_id:publicRoom.room_id})).deliveries.length>0);
    await page.locator(action('enable')).click();
    await page.locator(`${action('pause')}:not([disabled])`).waitFor();
  });
  await check('Room history and key metadata survive reconnecting a new candidate authority to PostgreSQL',async()=>{
    await call('fixture.hub.reopen',{},null);
    assert.ok((await call('agent.hub.console.rooms',{space:'private'})).rooms.some(row=>row.room_id===room.room_id));
    assert.equal((await remote({kind:'directory',space:'private'},secondKey)).ok,true);
    await go('delivery',`&room=${room.room_id}`);await page.getByText('Human approval from the browser fixture.',{exact:true}).waitFor();
    assert.equal(await page.locator('[data-hub-secret]').count(),0);
  });
  await check('A failed refresh reports the unavailable server and recovers through Retry',async()=>{
    try {
      setOffline(true);await page.locator(action('refresh')).click();
      await page.locator('[data-hub-error],.hub-error,[role="alert"]').filter({hasText:/unavailable|failed|try again|could not/i}).first().waitFor();
    } finally {setOffline(false);}
    const retry=page.locator(action('retry'));
    if(await retry.count())await retry.click();else await page.locator(action('refresh')).first().click();
    await page.getByText('Human approval from the browser fixture.',{exact:true}).waitFor();
    assert.equal(await page.locator('[data-hub-secret]').count(),0);
  });
  if(process.env.AGENT_HUB_FORMAL==='1')await check('Fresh complete formal Agent hub matrix',async()=>{
    await verifyAgentHubFormal({base,root,out,temporary,browser,cookieFor,roomId:room.room_id,emptyRoomId:emptyRoom.room_id,publicRoomId:publicRoom.room_id,zeroRoomId:zeroRoom.room_id,fileMessageId:fileMessage.message_id});
  });
  // Verify every textual artifact produced by this extension is credential-free.
  await check('Retained acceptance evidence contains no generated transport credentials',async()=>{
    const inspect=async directory=>{for(const entry of await fs.readdir(directory,{withFileTypes:true})){const filename=path.join(directory,entry.name);if(entry.isDirectory())await inspect(filename);else if(/\.(?:json|jsonl|md|log|txt)$/.test(entry.name)){const text=await fs.readFile(filename,'utf8');assert.ok(!secrets.some(secret=>text.includes(secret)),`Credential-free evidence: ${entry.name}`);}}};
    await inspect(out);
  });
  await fs.writeFile(path.join(out,'agent-hub-journeys.json'),JSON.stringify({boundary:'real candidate edge, typed ControlPlane and isolated PostgreSQL; no installed daemon or live hub data',checks},null,2));
  const failures=checks.filter(row=>row.status==='failed');
  console.log(JSON.stringify({passed:checks.length-failures.length,failed:failures.length,failures:failures.map(({name,evidence})=>({name,evidence})),report:path.join(out,'agent-hub-journeys.json')}));
  process.exitCode=failures.length?1:0;
}
