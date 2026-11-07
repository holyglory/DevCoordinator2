import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { pageVerifier } from "../../../skills/formal-web-ui-verification/scripts/formal_web_ui_verify.mjs";

const require = createRequire(import.meta.url);
const moduleRoot = process.env.FORMAL_WEB_UI_PLAYWRIGHT_NODE_MODULES;
const { chromium } = moduleRoot
  ? require(`${moduleRoot}/playwright`)
  : require("../../../ci/playwright/node_modules/playwright");
let browser;

before(async () => { browser = await chromium.launch({ headless: true }); });
after(async () => { await browser?.close(); });

for (const positioned of [false,true]) for (const escaped of [false,true]) {
  test(`controls respect the popup's own containing box: positioned ${positioned}, escaped ${escaped}`,async()=>{
    const page=await browser.newPage({viewport:{width:390,height:844}});
    try {
      await page.setContent(`<style>body{margin:0;background:#fff;color:#111}main{padding:12px}#anchor{position:relative;width:30px;height:40px}#popup{position:${positioned?'absolute':'static'};width:280px;padding:10px}input{width:${escaped?400:200}px;box-sizing:border-box}</style><main><div id="anchor"><div id="popup"><input aria-label="Search" value="Review"></div></div></main>`);
      const report=await measure(page);
      const failures=report.findings.filter(item=>item.rule==='control-outside-container');
      assert.equal(failures.length>0,!positioned||escaped);
    } finally {await page.close();}
  });
}

function editorFixture(extraStyle = "") {
  return `<!doctype html><meta charset="utf-8"><style>
    body { margin: 0; color: #111; background: white; }
    main { padding: 12px; }
    .cm-scroller { display: flex; position: relative; width: 170px; height: 104px; overflow: auto; font: 14px/24px monospace; }
    .cm-gutters { position: sticky; left: 0; z-index: 2; flex: none; width: 42px; background: #eee; }
    .cm-gutterElement { height: 24px; }
    .cm-content { flex: none; }
    .cm-line { width: 820px; height: 24px; white-space: pre; }
    .long-line { box-sizing: border-box; padding-left: 560px; }
    ${extraStyle}
  </style><main><div class="cm-scroller">
    <div class="cm-gutters"><div class="cm-gutter">${Array.from({ length: 12 }, (_, index) => `<div class="cm-gutterElement">${index + 1}</div>`).join("")}</div></div>
    <div class="cm-content"><div class="cm-line long-line"><span id="probe-away">long_identifier</span></div>
    ${Array.from({ length: 11 }, (_, index) => `<div class="cm-line"><span id="source-${index}">logic</span> <span>signal_${index};</span></div>`).join("")}</div>
  </div></main>`;
}

async function scrollCoordinates(page) {
  return page.evaluate(() => {
    const entries = [{ selector: "window", left: window.scrollX, top: window.scrollY }];
    const roots = [document];
    while (roots.length) {
      for (const element of roots.shift().querySelectorAll("*")) {
        if (element.shadowRoot) roots.push(element.shadowRoot);
        if (element.scrollWidth > element.clientWidth || element.scrollHeight > element.clientHeight) {
          entries.push({ selector: element.id || element.className || element.tagName, left: element.scrollLeft, top: element.scrollTop });
        }
      }
    }
    return entries;
  });
}

async function measure(page, strictTruncation = false) {
  await page.evaluate((strictTruncation) => {
    window.__FORMAL_WEB_UI_CONFIG__ = { rules: { strictTruncation }, inspectThemePalette: false };
  }, strictTruncation);
  return page.evaluate(pageVerifier);
}

function occlusions(report) {
  return report.findings.filter((finding) => ["occluded", "partially-occluded"].includes(finding.rule));
}

for (const width of [390, 1440]) {
  for (const covered of [false, true]) {
    test(`wrapped inline metadata samples painted text fragments: width ${width}, covered ${covered}`, async () => {
      const page = await browser.newPage({ viewport: { width, height: 844 } });
      try {
        await page.setContent(`<!doctype html><style>
          body{margin:0;background:white;color:#111;font:13px/24.8px sans-serif}
          main{margin:24px 27px;width:334px}
          .badge{display:inline-block;box-sizing:border-box;height:25px;padding:0 6px;background:#ddd;vertical-align:baseline}
          .before{width:180px}.after{width:160px}
          #metadata{font-family:monospace;overflow-wrap:anywhere}
        </style><main><span class="badge before">Verified record</span> <span id="metadata">saved/decision identifier-fragment</span> <span class="badge after">Review state</span></main>`);
        const geometry = await page.locator("#metadata").evaluate((element, covered) => {
          const range = document.createRange();
          range.selectNodeContents(element);
          const fragments = [...range.getClientRects()].filter(rect => rect.width > 0.5 && rect.height > 0.5);
          const own = point => {
            const top = document.elementFromPoint(point.x, point.y);
            return top === element || element.contains(top) || top?.contains(element);
          };
          const rect = element.getBoundingClientRect();
          const insetX = Math.min(8, Math.max(2, rect.width / 4));
          const insetY = Math.min(8, Math.max(2, rect.height / 4));
          const unionPoints = [
            { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 },
            { x: rect.left + insetX, y: rect.top + insetY },
            { x: rect.right - insetX, y: rect.top + insetY },
            { x: rect.left + insetX, y: rect.bottom - insetY },
            { x: rect.right - insetX, y: rect.bottom - insetY },
          ];
          const visibleFragmentCenters = fragments.every(rect => own({ x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 }));
          const unrelatedUnionPoints = unionPoints.filter(point => !own(point)).length;
          if (covered) for (const rect of fragments) {
            const cover = document.createElement("div");
            cover.className = "text-cover";
            cover.style.cssText = `position:fixed;left:${rect.left}px;top:${rect.top}px;width:${rect.width}px;height:${rect.height}px;background:#222;z-index:2`;
            document.body.append(cover);
          }
          return { fragmentCount: fragments.length, visibleFragmentCenters, unrelatedUnionPoints, union: {x:rect.x,y:rect.y,width:rect.width,height:rect.height}, hits:unionPoints.map(point=>document.elementFromPoint(point.x,point.y)?.className) };
        }, covered);
        assert(geometry.fragmentCount >= 2, "fixture must wrap an inline element");
        assert(geometry.visibleFragmentCenters, "adjacent badges must leave every painted text fragment visible");
        assert(geometry.unrelatedUnionPoints >= 3, `fixture must expose the union-rectangle false positive: ${JSON.stringify(geometry)}`);
        const report = await measure(page);
        const findings = occlusions(report).filter(finding => finding.selector === "#metadata");
        if (covered) assert(findings.some(finding => finding.severity === "critical"), "a real cover over the text fragments must remain critical");
        else assert.deepEqual(findings, [], "adjacent badges in empty union-rectangle space do not cover text");
      } finally { await page.close(); }
    });
  }
}

for (const tag of ["button", "div"]) {
  test(`wrapped text does not shrink ${tag} occlusion to its text area`, async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    try {
      await page.setContent(`<!doctype html><style>
        body{margin:20px;color:#111;background:white;font:16px/20px sans-serif}
        #box{box-sizing:border-box;width:220px;height:100px;padding:0 130px 0 10px;font:inherit}
      </style><${tag} id="box">Wrapped box label</${tag}>`);
      const geometry = await page.locator("#box").evaluate(element => {
        const range = document.createRange();
        range.selectNodeContents(element);
        const fragments = [...range.getClientRects()];
        const rect = element.getBoundingClientRect();
        const cover = document.createElement("div");
        cover.style.cssText = `position:fixed;left:${rect.left + 100}px;top:${rect.top}px;width:${rect.width - 100}px;height:${rect.height}px;background:#222;z-index:2`;
        document.body.append(cover);
        return { fragmentCount: fragments.length, textUncovered: fragments.every(rect => document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2) === element) };
      });
      assert(geometry.fragmentCount >= 2 && geometry.textUncovered);
      const report = await measure(page);
      assert(occlusions(report).some(finding => finding.selector === "#box" && finding.severity === "critical"), "controls and block containers must retain box coverage");
    } finally { await page.close(); }
  });
}

for (const covered of [false, true]) {
  test(`fractional modal scroll edge checks reachable content: covered ${covered}`, async () => {
    const page = await browser.newPage({ viewport: { width: 927, height: 873 } });
    try {
      await page.setContent(`<!doctype html><style>
        body{margin:0;background:white;color:#111;font:13px sans-serif}
        dialog{padding:0;border:0;width:400px;overflow:hidden}
        dialog[open]{display:flex;flex-direction:column}
        header,footer{height:50px;flex:none;background:white}
        .body{height:120.5px;overflow:auto;flex:none;padding:0 20px}
        .spacer{height:117.25px}.links{height:40px;position:relative}
        #reachable-link{display:inline-flex;line-height:15px;height:15px;color:#0645ad}
        .cover{position:absolute;left:0;top:0;width:180px;height:15px;background:#222;z-index:2}
      </style><dialog><header>Venue details</header><div class="body"><div class="spacer"></div><div class="links"><a id="reachable-link" href="#community">Community board</a>${covered ? '<div class="cover"></div>' : ''}</div></div><footer>Previous and next</footer></dialog>`);
      await page.locator('dialog').evaluate(element => element.showModal());
      const before = await scrollCoordinates(page);
      const report = await measure(page);
      const findings = occlusions(report).filter(finding => finding.selector.includes('reachable-link'));
      assert.deepEqual(await scrollCoordinates(page), before, 'detector must restore modal scroll position');
      if (covered) assert.ok(findings.some(finding => finding.severity === 'critical'));
      else {
        assert.deepEqual(findings, []);
        await page.locator('#reachable-link').click();
        assert.equal(new URL(page.url()).hash, '#community');
      }
    } finally { await page.close(); }
  });
}

function contextualOverlayFixture({ declared = true, reason = "User-opened preview", innerCover = false, positioned = true } = {}) {
  return `<!doctype html><style>
    body{margin:0;background:white;color:#111}
    #background{position:absolute;left:30px;top:30px;width:150px;height:34px}
    #popup{position:${positioned ? "fixed" : "relative"};left:12px;top:12px;width:250px;height:180px;background:#eee;z-index:2}
    #popup h2{margin:0;padding:12px;font:16px sans-serif}
    #inside{position:absolute;left:18px;top:70px;width:150px;height:34px}
    #inner-cover{position:absolute;left:18px;top:70px;width:150px;height:34px;z-index:3;background:#333}
  </style><button id="background">Source action</button>
  <section id="popup" role="dialog" aria-label="Preview" ${declared ? `data-ui-contextual-overlay="${reason}"` : ""}>
    <h2>Preview</h2><button id="inside">Edit inputs</button>${innerCover ? '<div id="inner-cover"></div>' : ""}
  </section>`;
}

for (const width of [390, 1440]) {
  test(`contextual overlay only excuses declared background coverage: ${width}`, async () => {
    const page = await browser.newPage({ viewport: { width, height: 844 } });
    try {
      await page.setContent(contextualOverlayFixture());
      const report = await measure(page);
      assert.deepEqual(occlusions(report), []);
      assert(report.findings.some(finding => finding.rule === "allowed-contextual-overlay" && finding.selector.includes("background")));
    } finally { await page.close(); }
  });
  test(`contextual overlay still detects covered controls within it: ${width}`, async () => {
    const page = await browser.newPage({ viewport: { width, height: 844 } });
    try {
      await page.setContent(contextualOverlayFixture({ innerCover: true }));
      const report = await measure(page);
      assert(occlusions(report).some(finding => finding.selector.includes("inside") && finding.severity === "critical"));
    } finally { await page.close(); }
  });
}

for (const options of [{ declared: false }, { reason: "" }, { positioned: false }]) {
  test(`contextual overlay requires an explicit positioned declaration: ${JSON.stringify(options)}`, async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    try {
      await page.setContent(contextualOverlayFixture(options));
      const report = await measure(page);
      assert(occlusions(report).some(finding => finding.selector.includes("background")));
    } finally { await page.close(); }
  });
}

test("ordinary positioned content cannot declare itself a contextual surface", async () => {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  try {
    await page.setContent(contextualOverlayFixture().replace('role="dialog"', 'role="region"'));
    const report = await measure(page);
    assert(occlusions(report).some(finding => finding.selector.includes("background")));
  } finally { await page.close(); }
});

for (const width of [390, 1440]) {
  for (const axis of ["x", "y", "both"]) {
    test(`scrolled non-fixed editor content remains reachable: ${width}, ${axis}`, async () => {
      const page = await browser.newPage({ viewport: { width, height: 844 } });
      try {
        await page.setContent(editorFixture());
        await page.locator(".cm-scroller").evaluate((element, axis) => {
          element.scrollLeft = axis !== "y" ? 64 : 0;
          element.scrollTop = axis !== "x" ? 80 : 0;
        }, axis);
        const before = await scrollCoordinates(page);
        const report = await measure(page);
        assert.deepEqual(report.findings.filter(finding => finding.rule === "offcanvas-cut"), []);
        assert.deepEqual(await scrollCoordinates(page), before);
      } finally { await page.close(); }
    });
  }
}

for (const axis of ["x", "y"]) {
  test(`genuinely unreachable negative content is not excused by a scroll container: ${axis}`, async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    try {
      await page.setContent(`<!doctype html><style>body{margin:0;color:#111;background:white}.scroll{position:relative;margin:12px;width:200px;height:150px;overflow:auto}.content{width:600px;height:600px}.bad{position:absolute;left:${axis === "x" ? "-30px" : "10px"};top:${axis === "y" ? "-30px" : "10px"};width:90px;height:50px}</style><div class="scroll"><div class="content"><button class="bad">Unreachable action</button></div></div>`);
      const report = await measure(page);
      assert(report.findings.some(finding => finding.rule === "offcanvas-cut" && finding.severity === "critical" && finding.selector.includes("bad")), JSON.stringify(report.findings));
    } finally { await page.close(); }
  });
}

for (const width of [390, 1440]) {
  for (const left of [0, 64]) {
    test(`pinned gutter stays reachable and restores every scroll coordinate: width ${width}, left ${left}`, async () => {
      const page = await browser.newPage({ viewport: { width, height: 844 } });
      try {
        await page.setContent(editorFixture());
        await page.locator(".cm-scroller").evaluate((element, offset) => { element.scrollLeft = offset; }, left);
        const before = await scrollCoordinates(page);
        const report = await measure(page);
        const after = await scrollCoordinates(page);
        assert.deepEqual({ scroll: after, occlusions: occlusions(report) }, { scroll: before, occlusions: [] });
      } finally {
        await page.close();
      }
    });
  }
}

test("nested and shadow-root scroll positions survive probes and smooth CSS", async () => {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  try {
    await page.setContent(editorFixture("html { scroll-behavior: smooth; } body { width: 1100px; height: 1700px; } .cm-scroller { scroll-behavior: smooth; }"));
    await page.evaluate((fixture) => {
      const host = document.createElement("section");
      host.id = "shadow-editor";
      host.style.cssText = "display:block; width:220px; height:120px; overflow:auto; margin:24px 180px";
      document.body.append(host);
      host.attachShadow({ mode: "open" }).innerHTML = fixture;
      host.shadowRoot.querySelector(".cm-scroller").scrollTo({ left: 64, top: 48, behavior: "instant" });
      host.scrollTo({ left: 0, top: 10, behavior: "instant" });
      document.querySelector(".cm-scroller").scrollTo({ left: 64, top: 24, behavior: "instant" });
      window.scrollTo({ left: 110, top: 20, behavior: "instant" });
    }, editorFixture());
    const before = await scrollCoordinates(page);
    await measure(page);
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.deepEqual(await scrollCoordinates(page), before);
  } finally {
    await page.close();
  }
});

for (const obstruction of ["oversized-gutter", "absolute-overlay", "fixed-overlay", "non-edge-sticky"]) {
  test(`unreachable source is still critical: ${obstruction}`, async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    try {
      await page.setContent(editorFixture(obstruction === "oversized-gutter" ? ".cm-gutters { width: 190px; }" : ""));
      if (obstruction !== "oversized-gutter") {
        await page.evaluate((kind) => {
          const source = document.querySelector("#source-0");
          const rect = source.getBoundingClientRect();
          const cover = document.createElement("div");
          cover.id = "unrelated-cover";
          if (kind === "fixed-overlay") {
            cover.style.cssText = `position:fixed;left:${rect.left}px;top:${rect.top}px;width:85px;height:24px;background:#ddd;z-index:20`;
            document.body.append(cover);
          } else {
            cover.style.cssText = `position:${kind === "non-edge-sticky" ? "sticky" : "absolute"};left:42px;top:24px;width:85px;height:24px;background:#ddd;z-index:20;flex:none;margin-left:-820px`;
            if (kind === "absolute-overlay") cover.style.marginLeft = "0";
            document.querySelector(".cm-scroller").append(cover);
          }
        }, obstruction);
      }
      const report = await measure(page);
      assert(occlusions(report).some((finding) => finding.severity === "critical" && finding.selector.startsWith("#source-")), JSON.stringify(report.findings));
    } finally {
      await page.close();
    }
  });
}

test("non-scrollable clipped content stays critical", async () => {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  try {
    await page.setContent(`<!doctype html><style>
      body{margin:0;color:#111;background:white;font:11px/14.7px Arial}
      main{padding:12px}.crop{width:70px;overflow:hidden;white-space:nowrap;margin-bottom:12px}
      button{width:36px;height:36px;position:relative}
      .label{position:absolute;width:1px;height:1px;clip:rect(0,0,0,0);overflow:hidden;white-space:nowrap}
    </style><main>
      <div class="crop"><span id="real-clipped">source_identifier_that_cannot_be_reached</span></div>
      <button aria-label="Elaborate">?<span class="label"><span id="hidden-label">Elaborate</span></span></button>
      <div class="crop"><button id="clipped-control" style="width:120px">Visible action</button></div>
      <div class="crop" style="position:absolute;top:160px;height:18px;clip:rect(0,70px,18px,0)"><span id="partial-legacy-clip">another_identifier_that_is_partially_painted</span></div>
      <div style="clip:rect(0,0,0,0)"><span id="inactive-clip">Visible sibling label</span></div>
    </main>`);
    const report = await measure(page);
    for (const selector of ["#real-clipped", "#clipped-control", "#partial-legacy-clip"]) {
      assert(report.findings.some((finding) => finding.selector === selector && finding.severity === "critical" && finding.rule === "clipped-by-ancestor"));
    }
    assert.equal(report.findings.filter((finding) => finding.selector === "#hidden-label").length, 0,
      "a nested label inside a zero-area CSS clip does not paint");
    assert(report.metrics.hiddenTextLike.zeroSize >= 2, "hidden parent and child remain counted");
    assert.equal(report.findings.filter((finding) => finding.selector === "#inactive-clip").length, 0,
      "clip on a static box does not hide a visible descendant");
    for (const variant of ["fixed", "absolute", "contents-absolute", "static-absolute"]) {
      await page.setContent(`<!doctype html><style>
        body{margin:0;background:white;color:#111}main{position:relative;width:390px;height:340px}
        .owner{position:${variant === "static-absolute" ? "static" : "absolute"};display:${variant === "contents-absolute" ? "contents" : "block"};width:1px;height:1px;left:10px;top:10px;clip:rect(0,0,0,0);overflow:hidden}
        button{position:${variant === "fixed" ? "fixed" : "absolute"};left:150px;top:150px;width:150px;height:50px}
      </style><main><div class="owner"><button id="escaped-control">Visible action</button></div></main>`);
      const painted = await page.locator("#escaped-control").evaluate(el => {
        const rect = el.getBoundingClientRect();
        return document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2) === el;
      });
      const escapes = variant === "contents-absolute" || variant === "static-absolute";
      assert.equal(painted, escapes, `${variant} must agree with actual browser hit-testing`);
      if (escapes) {
        await page.locator("#escaped-control").evaluate(el => {
          const rect = el.getBoundingClientRect();
          const cover = document.createElement("div");
          cover.style.cssText = `position:fixed;left:${rect.left}px;top:${rect.top}px;width:${rect.width}px;height:${rect.height}px;background:#ccc;z-index:10`;
          document.body.append(cover);
        });
      }
      const measured = await measure(page);
      const failures = measured.findings.filter(item => item.selector === "#escaped-control");
      if (escapes) assert(failures.some(item => item.rule === "occluded" && item.severity === "critical"), `${variant} stays checked after it visibly escapes`);
      else assert.equal(failures.length, 0, `${variant} is actually hidden by the ancestor clip`);
    }
  } finally {
    await page.close();
  }
});

test("a failing probe still restores every scroll coordinate", async () => {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  try {
    await page.setContent(editorFixture());
    const before = await scrollCoordinates(page);
    await page.evaluate(() => {
      const original = Element.prototype.scrollIntoView;
      Element.prototype.scrollIntoView = function (...argumentsList) {
        original.apply(this, argumentsList);
        throw new Error("intentional-probe-failure");
      };
    });
    await assert.rejects(measure(page), /intentional-probe-failure/);
    assert.deepEqual(await scrollCoordinates(page), before);
  } finally {
    await page.close();
  }
});

test("an edge-pinned gutter cannot excuse source outside its scroll range", async () => {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  try {
    await page.setContent(editorFixture(".cm-content { margin-left: -42px; }"));
    const report = await measure(page);
    assert(occlusions(report).some((finding) => finding.severity === "critical" && finding.selector === "#source-0"));
  } finally {
    await page.close();
  }
});

test("right-pinned sibling reachability preserves a nonzero offset", async () => {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  try {
    await page.setContent(editorFixture(".cm-gutters { left: auto; right: 0; order: 2; }"));
    await page.locator(".cm-scroller").evaluate((element) => { element.scrollLeft = 495; });
    const before = await scrollCoordinates(page);
    const report = await measure(page);
    assert.deepEqual({ scroll: await scrollCoordinates(page), occlusions: occlusions(report) }, { scroll: before, occlusions: [] });
  } finally {
    await page.close();
  }
});

const fixturesRoot = new URL("../../../skills/formal-web-ui-verification/fixtures/self-test/", import.meta.url);
const pages = JSON.parse(fs.readFileSync(new URL("pages.json", fixturesRoot), "utf8"));
const matrix = JSON.parse(fs.readFileSync(new URL("matrix.json", fixturesRoot), "utf8"));
for (const covered of [false, true]) {
  test(`document popup probe preserves the original heading: covered ${covered}`, async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 921 } });
    try {
      await page.setContent(pages[`popup-scroll-probe-${covered ? "covered" : "clean"}.html`]);
      const before = await scrollCoordinates(page);
      const report = await measure(page);
      assert.deepEqual(await scrollCoordinates(page), before);
      if (covered) {
        assert(occlusions(report).some((finding) => finding.selector === "#page-heading" && finding.severity === "critical"));
      } else {
        assert.equal(occlusions(report).length, 0);
      }
    } finally {
      await page.close();
    }
  });
}

test("the gutter itself is not exempt from unrelated occlusion", async () => {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  try {
    await page.setContent(editorFixture());
    await page.evaluate(() => {
      const gutter = document.querySelector(".cm-gutterElement");
      gutter.id = "protected-gutter";
      const rect = gutter.getBoundingClientRect();
      const cover = document.createElement("div");
      cover.style.cssText = `position:fixed;left:${rect.left}px;top:${rect.top}px;width:${rect.width}px;height:${rect.height}px;background:#ddd;z-index:20`;
      document.body.append(cover);
    });
    const report = await measure(page);
    assert(occlusions(report).some((finding) => finding.selector === "#protected-gutter" && finding.severity === "critical"));
  } finally {
    await page.close();
  }
});

const selectedCases = new Set(["occluded", "partial-overlap", "overlay-in-scroll", "scroll-panel-neighbor", "details-closed-neighbor", "dev-overlay-badge", "modal-active-occlusion", "modal-shadow-occlusion", "modal-scroll-locked", "fixed-unscrollable-cut", "truncate-inner-span"]);
for (const fixture of matrix.cases.filter((entry) => selectedCases.has(entry.name) || entry.name.startsWith("custom-modal-"))) {
  test(`existing detector guard: ${fixture.name}`, async () => {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    try {
      await page.setContent(pages[fixture.path]);
      const report = await measure(page);
      for (const rule of fixture.critical_rules || []) {
        assert(report.findings.some((finding) => finding.rule === rule && finding.severity === "critical"), `${fixture.name} missed ${rule}`);
      }
      for (const rule of fixture.forbidden_rules || []) {
        assert(!report.findings.some((finding) => finding.rule === rule), `${fixture.name} falsely reported ${rule}`);
      }
      if (!fixture.critical) assert(!report.findings.some((finding) => finding.severity === "critical"), JSON.stringify(report.findings));
      if (fixture.name === "truncate-inner-span") {
        assert(report.metrics.ellipsisTruncations.some(item => item.selector === "#compact-identifier"));
        const strict = await measure(page, true);
        assert(strict.findings.some(item => item.selector === "#compact-identifier" && item.rule === "clipped-by-ancestor" && item.severity === "critical"));
        for (const changed of ["height", "position", "control", "shadow", "decoration", "font-mismatch", "unsupported-metrics"]) {
          await page.setContent(pages[fixture.path]);
          await page.evaluate((changed) => {
            const cell = document.querySelector(".compact-cell");
            const text = document.querySelector("#compact-identifier");
            if (changed === "height") cell.style.height = "5px";
            else if (changed === "position") { text.style.position = "relative"; text.style.top = "-6px"; }
            else if (changed === "control") { const button = document.createElement("button"); button.id = text.id; button.textContent = text.textContent; text.replaceWith(button); }
            else if (changed === "shadow") cell.style.textShadow = "0 3px 1px black";
            else if (changed === "decoration") cell.style.textDecoration = "underline";
            else if (changed === "font-mismatch") text.style.fontSize = "13px";
            else {
              const original = CanvasRenderingContext2D.prototype.measureText;
              CanvasRenderingContext2D.prototype.measureText = function (...args) {
                const metrics = original.apply(this, args);
                return new Proxy(metrics, { get(target, key) { return key === "actualBoundingBoxAscent" ? Number.NaN : Reflect.get(target, key, target); } });
              };
            }
          }, changed);
          const clipped = await measure(page);
          assert(clipped.findings.some(item => item.selector === "#compact-identifier" && item.rule === "clipped-by-ancestor" && item.severity === "critical"), `${changed} must not inherit an ellipsis exception`);
        }
      }
    } finally {
      await page.close();
    }
  });
}

test("retained candidate CLI verifies the rendered editor at both widths", () => {
  const evidence = fs.mkdtempSync(path.join(os.tmpdir(), "formal-occlusion-proof-"));
  const fixture = path.join(evidence, "editor.html");
  const config = path.join(evidence, "config.json");
  const reportPath = path.join(evidence, "report.json");
  const root = fileURLToPath(new URL("../../../", import.meta.url));
  fs.writeFileSync(fixture, editorFixture());
  fs.writeFileSync(config, JSON.stringify({
    repoRoot: root,
    targetDefaults: {
      journeys: [{ id: "source", frequencyPercent: 100, risk: "normal" }],
      primaryJourney: "source",
      regions: [{ selector: "main", role: "primary-content", journey: "source" }],
      theme: "light",
      reviewInputs: [{ path: "rust/tooling/tests/formal_occlusion.mjs", kind: "ui-code" }],
    },
    targets: [{ name: "pinned-source", url: pathToFileURL(fixture).href }],
    viewports: [{ name: "compact", width: 390, height: 844 }, { name: "wide", width: 1440, height: 900 }],
    scroll: false,
  }));
  const environment = { ...process.env, TMPDIR: evidence };
  delete environment.DEVCOORDINATOR_EVIDENCE_DIR;
  delete environment.DEVCOORDINATOR_RUN_ID;
  delete environment.DEVCOORDINATOR_CHECK_NAME;
  const run = spawnSync(process.execPath, [
    path.join(root, "skills/formal-web-ui-verification/scripts/formal_web_ui_verify.mjs"),
    "--config", config, "--json-out", reportPath, "--markdown-out", path.join(evidence, "report.md"),
    "--playwright-module-dir", moduleRoot || path.join(root, "ci/playwright/node_modules"),
  ], { encoding: "utf8", env: environment, timeout: 60000 });
  fs.writeFileSync(path.join(evidence, "stdout.json"), run.stdout || "");
  fs.writeFileSync(path.join(evidence, "stderr.log"), run.stderr || "");
  assert.equal(run.status, 0, `retained verifier failed; evidence: ${evidence}`);
  const report = JSON.parse(fs.readFileSync(reportPath, "utf8"));
  assert.equal(report.pages.length, 2);
  assert.equal(occlusions(report).length, 0);
  assert(!report.findings.some((finding) => finding.severity === "critical"));
  process.stdout.write(`Retained CLI proof: ${evidence}\n`);
});
