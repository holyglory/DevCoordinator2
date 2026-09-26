// Find likely mixed-language product messages. This is a review aid, not a
// language classifier: brand names, identifiers and user-authored data stay out
// of the catalogs and are not examined here.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../',import.meta.url));
const localeRoot=path.join(root,'console/locales');
const manifest=JSON.parse(await fs.readFile(path.join(localeRoot,'manifest.json')));
const requested=process.argv.includes('--locale')?process.argv[process.argv.indexOf('--locale')+1]:null;
const stopWords=['the','and','or','why','needs','you','your','could','not','load','loading','available','unavailable','response','history','is','are','to','view','show','open','remove','critical','dismissed','incident','reported','health','system','trends','host','capacity','search','find','details','logs','next','step','active','dismiss','restore','managed','storage','memory','free','average','scale','this','that','with','for','from','of','in','on','by','before','after','still','here','finish','action','already','recorded','complete','current','previous','matching','period','plan','task','repository','resources','review','refresh','try','again'];
const phrasePatterns=[/your comment and marks are still here/i,/to attach this comment/i,/the text label before saving/i,/wait for it to finish/i,/could not be confirmed/i,/is available/i,/is unavailable/i,/needs attention/i,/the stored incident/i,/open plan task/i,/view delivery evidence/i,/loading screenshot/i,/no .* available/i,/this project uses shared/i,/interactive plan timeline/i,/showing .* of .* open tasks/i];
const tokenRe=/[A-Za-z][A-Za-z'-]*/g;
let totalCandidates=0;
const entries=manifest.locales.filter(e=>e.tag!=='en'&&(!requested||e.tag===requested));
for(const entry of entries){const matches=[]; for(const file of Object.values(entry.files).flat()){const ns=path.basename(file,'.json'); const data=JSON.parse(await fs.readFile(path.join(localeRoot,file),'utf8')); for(const [id,msg] of Object.entries(data)){const vals=typeof msg==='string'?[msg]:Object.values(msg?.forms||{}); for(const value of vals){if(typeof value!=='string')continue; const words=(value.match(tokenRe)||[]).map(x=>x.toLowerCase()).filter(x=>stopWords.includes(x)); const phrase=phrasePatterns.some(p=>p.test(value)); if(phrase||new Set(words).size>=3)matches.push({namespace:ns,id,value,reason:phrase?'phrase':'english-token-cluster'});}}} totalCandidates+=matches.length; console.log(JSON.stringify({locale:entry.tag,status:entry.status,candidates:matches.length,matches},null,2));}
if(process.argv.includes('--strict') && totalCandidates) process.exitCode=1;
