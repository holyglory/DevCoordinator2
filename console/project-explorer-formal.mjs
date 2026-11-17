// Source-owned contract for the complete explorer's browser verification.
// Auth cookies are supplied only in memory by the isolated acceptance harness.
export function explorerFormalConfig({root,base,projectId,theme,playwrightModuleDir,cookie}) {
  const views={copies:'.pe-trees',branches:'.pe-trees',overview:'.pe-overview',storage:'.pe-storage',history:'.pe-history'};
  const geometry=(primary,heading='[data-pe-title]',identifier='.pe-branch .pe-node-name')=>[
    {id:'content-width',kind:'primary-content-width',selector:primary,minWidth:290},
    {id:'heading',kind:'readable-heading',selector:heading},
    {id:'identity',kind:'readable-canonical-identifier',selector:identifier},
    {id:'word-wrap',kind:'no-character-wrapping',selector:heading},
    {id:'document-width',kind:'document-horizontal-overflow',selector:'html'},
    {id:'primary-visible',kind:'initial-viewport-placement',selector:primary},
    {id:'primary-clipping',kind:'clipping',selector:primary},
  ];
  const targets=Object.entries(views).map(([view,selector])=>({
    name:`project-${view}-${theme}`,url:`${base}#/projects/${encodeURIComponent(projectId)}?view=${view}`,theme,
    journeys:[{id:'explore',name:'Understand project branches, copies, lifecycle and space',frequencyPercent:85,risk:'normal',rationale:'Most visits inspect current project work.'},{id:'edit',name:'Edit branch intent and outcome',frequencyPercent:10,risk:'normal',rationale:'Operators maintain branch meaning.'},{id:'remove',name:'Review local removal and data loss',frequencyPercent:5,risk:'high',rationale:'Removal must describe exact effects before execution.'}],
    primaryJourney:'explore',regions:[{name:'Project content',selector,role:'primary-content',journey:'explore'},{name:'Project navigation',selector:'.pe-heading',role:'supporting'}],
    reviewInputs:[{path:'console/project-explorer.js',kind:'ui-code'},{path:'console/project-explorer.css',kind:'style'},{path:'console/workspace.js',kind:'ui-code'},{path:'console/index.html',kind:'ui-code'},{path:'console/design-system.css',kind:'tokens'},{path:'console/locales/en/projects.json',kind:'ui-code'},{path:'console/icons/tabler',kind:'asset'}],
    waitFor:{selector,renderFrames:2,timeoutMs:8000},
    geometryAssertions:geometry(selector,'[data-pe-title]',view==='copies'||view==='branches'?'.pe-branch .pe-node-name':'[data-pe-title]'),
    screenshotMasks:[{selector:'#who-email',reason:'Signed-in account is private.'}],
    states:[],
  }));
  const copies=targets[0];
  copies.breakpointProfile={name:'explorer',breakpoints:[430,760,1050,1200,1800],height:1058};
  copies.states=[
    {name:'commits',actions:[{action:'check',selector:'[data-pe-show-commits]'}],waitFor:{selector:'.pe-commit',renderFrames:2,timeoutMs:10000}},
    {name:'edit',actions:[{action:'click',selector:'[data-pe-edit]'}],primaryJourney:'edit',priorityOverrideReason:'Operator opened the selected item editor.',regions:[{name:'Metadata editor',selector:'.pe-metadata-dialog',role:'primary-content',journey:'edit'}],waitFor:{selector:'[data-pe-editor] textarea:not([readonly])',renderFrames:2},geometryAssertions:geometry('.pe-metadata-dialog','[data-pe-editor] h2','[data-pe-editor] header span'),continuation:{kind:'in-page',anchor:'[data-pe-editor] h2',focusWithin:'[data-pe-editor]',maxScrollDelta:8},cleanup:{actions:[{action:'click',selector:'[data-pe-cancel-edit]'}]}},
    {name:'remove',actions:[{action:'click',selector:'[data-pe-remove]'}],primaryJourney:'remove',priorityOverrideReason:'Operator opened the exact removal plan.',regions:[{name:'Removal plan',selector:'.pe-removal-dialog',role:'primary-content',journey:'remove'}],waitFor:{selector:'.pe-removal-dialog[open]',renderFrames:2},geometryAssertions:geometry('.pe-removal-dialog','#pe-removal-title','.pe-removal-targets strong'),continuation:{kind:'in-page',anchor:'#pe-removal-title',focusWithin:'.pe-removal-dialog',maxScrollDelta:8},cleanup:{actions:[{action:'click',selector:'.pe-removal-dialog [data-close]'}]}}
  ];
  const viewports=[{name:'phone',device:'iPhone 13',colorScheme:theme},{name:'intermediate',width:900,height:1024,colorScheme:theme},{name:'desktop',width:1487,height:1058,colorScheme:theme},{name:'wide',width:1920,height:1080,colorScheme:theme}];
  const requiredCoverage=targets.flatMap(target=>['base',...target.states.map(s=>s.name)].flatMap(state=>viewports.map(viewport=>({target:target.name,state,viewport:viewport.name}))));
  const fixtureDataShapes=targets.flatMap(target=>['base',...target.states.map(s=>s.name)].map(state=>({id:`${target.name}-${state}`,revision:'project-explorer-v1',target:target.name,route:'/',state,conditionalDom:state==='edit'?['.pe-metadata-dialog']:state==='remove'?['.pe-removal-dialog']:state==='commits'?['.pe-commit']:[target.regions[0].selector],layoutEffect:'Real project API data includes two repositories, observed branches, dirty primary and linked feature checkout. Optional commit and exact removal/editor states use their actual handlers.'})));
  return {repoRoot:root,playwrightModuleDir,cookies:[cookie],targets,viewports,requiredCoverage,fixtureDataShapes,maxPageCount:128};
}
