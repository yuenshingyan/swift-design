// Run after building the debug server and UI. Requires Node 22+ and Chrome.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { homedir, tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const root = process.cwd();
const scratch = mkdtempSync(join(tmpdir(), 'swift-screen-review-'));
const [candidateFile] = process.argv.slice(2);
const session = 'screen-review';
const id = `${session}-candidate-1`;
const designRoute = `/designs/${id}`;
let app, server, browser, socket;
let stage = 'starting', unsafe = false, rejectedMarkup = false;
const requests = [];
const logs = [];
const original = JSON.parse(readFileSync('fixtures/sample-design.json', 'utf8'));
original.theme.fonts = { heading: 'Arial', body: 'Arial', mono: 'Courier New' };
original.screens = [
  { name: 'Home', html: "<h1>Home</h1><p>Workspace summary</p><a href='#screen-2'>Members</a>",
    css: 'h1{position:absolute;left:80px;top:100px;width:720px;font-size:42px}p{position:absolute;left:560px;top:100px;font-size:42px}a{position:absolute;left:80px;top:240px}' },
  { name: 'Members', html: '<h1>Members</h1><p>Review access for this project.</p>' },
  { name: 'Settings', html: '<h1>Settings</h1><p>This screen should stay unchanged.</p>' },
];
const replacements = [
  { index: 0, screen: { ...original.screens[0], html: original.screens[0].html.replace('Home</h1>', 'Overview</h1>') } },
  { index: 1, screen: { ...original.screens[1], html: original.screens[1].html.replace('Members</h1>', 'Project members</h1>') } },
];

const model = createServer(async (req, res) => {
  try {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks));
    const system = body.messages[0].content;
    let reply;
    if (system.includes('This is a capability check')) {
      reply = { unsupported_requirements: [] };
    } else if (system.includes('Review demo requirement coverage')) {
      assert(!unsafe, 'unsafe markup must never reach requirement review');
      reply = { coverage: [{ complete: true }, { complete: true }] };
    } else {
      requests.push(body.messages[1].content);
      rejectedMarkup ||= unsafe && body.messages.some(message => message.role === 'user'
        && /failed validation:[\s\S]*script/.test(String(message.content)));
      reply = unsafe ? { screens: [{ index: 0, screen: { ...replacements[0].screen,
        html: replacements[0].screen.html + '<script>void 0</script>' } }] } : { screens: replacements };
    }
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ id: 'mock', choices: [{ message: { role: 'assistant', content: JSON.stringify(reply) } }] }));
  } catch (error) {
    res.writeHead(500);
    res.end(String(error));
  }
});

function listen(listener) {
  return new Promise((done, fail) => listener.once('error', fail).listen(0, '127.0.0.1', () => done(listener.address().port)));
}

async function until(check, timeout = 30_000) {
  const deadline = Date.now() + timeout;
  do {
    const value = await check();
    if (value) return value;
    await sleep(100);
  } while (Date.now() < deadline);
  assert.fail(`Timed out during ${stage}`);
}

async function api(path, method = 'GET', body, status = 200) {
  const response = await fetch(`${app}${path}`, {
    method, headers: { 'Content-Type': 'application/json', 'x-swift-design-author': 'user' },
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(90_000),
  });
  const bytes = Buffer.from(await response.arrayBuffer());
  assert.equal(response.status, status, `${method} ${path}: ${bytes.toString().slice(0, 400)}`);
  return response.headers.get('content-type')?.includes('application/json') ? JSON.parse(bytes) : bytes;
}

const pending = new Map();
let command = 0;
function cdp(method, params = {}) {
  return new Promise((done, fail) => {
    const id = ++command;
    pending.set(id, { done, fail });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const reply = await cdp('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  assert(!reply.exceptionDetails, reply.exceptionDetails?.exception?.description ?? expression);
  return reply.result.value;
}
const wait = (expression) => until(() => evaluate(expression));
const button = (text) => `Array.from(document.querySelectorAll('button')).find(el=>el.textContent.trim()===${JSON.stringify(text)})`;
const surface = "document.querySelector('.annotation-surface')";
const pane = "document.querySelector('.preview-swap iframe[data-preview=true]')";
const badges = `(Array.from(document.querySelectorAll('.demo-thumbnails .thumbnail')).map(el=>el.querySelector('.thumbnail-change')?.textContent ?? ''))`;
const editor = () => cdp('Page.navigate', { url: `${app}/#/designs/${id}` });
const selectScreen = (index) => evaluate(`document.querySelector('.demo-thumbnails [data-index="${index}"]').click()`);

async function draw(start, end, number, comment) {
  await wait(`!!${surface}`);
  await evaluate(`${surface}.scrollIntoView({block:'center'})`);
  const r = await evaluate(`${surface}.getBoundingClientRect().toJSON()`);
  const dispatch = (type, point, buttons) => cdp('Input.dispatchMouseEvent', {
    type, button: 'left', buttons, clickCount: 1, x: r.left + r.width * point[0], y: r.top + r.height * point[1],
  });
  await dispatch('mousePressed', start, 1);
  await dispatch('mouseMoved', end, 1);
  await dispatch('mouseReleased', end, 0);
  const textarea = `document.querySelector('textarea[aria-label="Comment for box ${number}"]')`;
  await wait(`!!${textarea}`);
  await evaluate(`${textarea}.focus()`);
  await cdp('Input.insertText', { text: comment });
}

try {
  const modelPort = await listen(model);
  const reservation = createServer();
  const port = await listen(reservation);
  await new Promise(resolve => reservation.close(resolve));
  app = `http://127.0.0.1:${port}`;
  const chrome = process.env.SWIFT_DESIGN_CHROME || join(homedir(), '.local/share/swift-design/chrome/chrome-linux64/chrome');
  for (const path of [chrome, 'target/debug/server', 'target/dx/ui/debug/web/public/index.html']) {
    assert(existsSync(path), `Required for this test: ${path}`);
  }
  const sessionDir = join(scratch, 'data/sessions', session);
  mkdirSync(sessionDir, { recursive: true });
  const time = new Date().toISOString();
  writeFileSync(join(sessionDir, 'session.json'), JSON.stringify({ id: session, title: 'Screen review',
    request: 'Create a small review prototype.', artifact_kind: 'demo', state: 'reviewing', created_at: time, updated_at: time }));
  // Do not inherit credentials, production storage paths, or a configured custom runner.
  server = spawn(resolve('target/debug/server'), [], { cwd: scratch, env: {
    PATH: process.env.PATH, HOME: process.env.HOME, SWIFT_DESIGN_ADDRESS: `127.0.0.1:${port}`,
    SWIFT_DESIGN_UI_DIR: resolve('target/dx/ui/debug/web/public'), SWIFT_DESIGN_PROVIDER: 'mock',
    SWIFT_DESIGN_PROVIDER_URL: `http://127.0.0.1:${modelPort}/v1/chat/completions`, SWIFT_DESIGN_MODEL: 'gpt-6-test', SWIFT_DESIGN_CHROME: chrome,
  } });
  for (const stream of [server.stdout, server.stderr]) stream.on('data', data => { logs.push(String(data)); });
  await until(() => fetch(`${app}/health`).then(r => r.ok).catch(() => false));
  for (const screens of [original.screens.slice(0, 2), original.screens]) {
    await api(designRoute, 'PUT', { ...original, screens }, 204);
  }

  const profile = join(scratch, 'chrome');
  browser = spawn(chrome, ['--headless=new', '--no-sandbox', '--disable-dev-shm-usage', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
  const portFile = join(profile, 'DevToolsActivePort');
  await until(() => existsSync(portFile));
  const debugPort = readFileSync(portFile, 'utf8').split('\n')[0];
  const pages = await fetch(`http://127.0.0.1:${debugPort}/json/list`).then(r => r.json());
  socket = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl);
  await new Promise((done, fail) => { socket.onopen = done; socket.onerror = fail; });
  socket.onmessage = ({ data }) => {
    const { id, result, error } = JSON.parse(data);
    const waiter = pending.get(id);
    if (!waiter) return;
    pending.delete(id);
    if (error) waiter.fail(Error(error.message)); else waiter.done(result);
  };
  await cdp('Page.enable');
  await cdp('Network.enable');
  await cdp('Network.setBlockedURLs', { urls: ['https://fonts.googleapis.com/*', 'https://fonts.gstatic.com/*'] });
  const resize = (width, height) => cdp('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  await resize(1440, 1000);

  stage = 'screen groups, search, navigation, and export selection';
  const groupedRoute = '/designs/screen-review-candidate-2';
  const grouped = original.screens.map((screen, index) => ({
    ...screen, name: ['Overview · Home', 'Access · Members', 'Access · Settings'][index],
  }));
  await api(groupedRoute, 'PUT', { ...original, screens: grouped.slice(0, 2) }, 204);
  await api(groupedRoute, 'PUT', { ...original, screens: grouped }, 204);
  await cdp('Page.navigate', { url: `${app}/#${groupedRoute}` });
  const access = `Array.from(document.querySelectorAll('.screen-group-toggle')).find(el=>el.querySelector('.screen-group-name').textContent==='Access')`;
  await wait(`${access}?.getAttribute('aria-expanded')==='false'`);
  assert.match(await evaluate(`${access}.textContent`), /2 screens.*New/);
  const accessCheckbox = `${access}.parentElement.querySelector('[role=checkbox]')`;
  const accessState = `${accessCheckbox}.getAttribute('aria-checked')`;
  const homeCheckbox = `document.querySelector('.thumbnail[data-index="0"] input')`;
  const membersCheckbox = `document.querySelector('.thumbnail[data-index="1"] input')`;
  assert.equal(await evaluate(accessState), 'false');
  assert.equal(await evaluate("document.querySelectorAll('.demo-thumbnails iframe').length"), 1);
  await evaluate(`${homeCheckbox}.click()`);
  const search = "document.querySelector('.screen-search')";
  await evaluate(`${search}.focus()`);
  await cdp('Input.insertText', { text: 'SETTINGS' });
  const settingsTile = `document.querySelector('.thumbnail[data-index="2"]')`;
  await wait(`${settingsTile}?.textContent.includes('New')`);
  assert.equal(await evaluate("document.querySelectorAll('.demo-thumbnails iframe').length"), 1);
  await evaluate(`${settingsTile}.querySelector('input').click()`);
  await wait(`${accessState}==='mixed'`);
  await evaluate(`${accessCheckbox}.focus()`);
  for (const type of ['keyDown', 'keyUp']) {
    await cdp('Input.dispatchKeyEvent', { type, key: ' ', code: 'Space', windowsVirtualKeyCode: 32 });
  }
  await wait(`${accessState}==='true'`);
  await evaluate(`${search}.value=''; ${search}.dispatchEvent(new Event('input', {bubbles:true}))`);
  await wait(`${access}?.getAttribute('aria-expanded')==='false'`);
  await evaluate(`${access}.click()`);
  await wait(`${settingsTile}?.querySelector('input').checked && ${membersCheckbox}.checked`);
  await evaluate(`${access}.click()`);
  await evaluate(`${accessCheckbox}.click()`);
  await wait(`${accessState}==='false' && ${access}.getAttribute('aria-expanded')==='false'`);
  assert(await evaluate(`${homeCheckbox}.checked`), 'clearing Access must preserve the Overview selection');
  await evaluate(`${accessCheckbox}.click()`);
  await wait(`${accessState}==='true'`);
  await wait(`!!${pane}.contentDocument?.querySelector('a[href="#screen-2"]')`);
  await evaluate(`${pane}.contentDocument.querySelector('a[href="#screen-2"]').click()`);
  await wait(`${access}?.getAttribute('aria-expanded')==='true' && ${pane}.src.endsWith('screen=2')`);
  assert(await evaluate(`${settingsTile}.querySelector('input').checked`));

  stage = 'initial badges and drawing';
  await cdp('Page.navigate', { url: 'about:blank' });
  await editor();
  await wait(`${badges}.join(',') === ',,New'`);
  await wait(`!!${pane}.contentDocument?.querySelector('[data-swift-design-root] h1')`);
  assert(await evaluate(`(() => {
    const doc = ${pane}.contentDocument;
    const heading = doc.querySelector('[data-swift-design-root] h1'), paragraph = doc.querySelector('[data-swift-design-root] p');
    const text = doc.createRange(); text.selectNodeContents(heading);
    return heading.getBoundingClientRect().right > paragraph.getBoundingClientRect().left
      && text.getBoundingClientRect().right < paragraph.getBoundingClientRect().left;
  })()`), 'the overlap fixture must have intersecting boxes but separate text');
  await evaluate(`${button('Annotate')}.click()`);
  await draw([0.05, 0.1], [0.24, 0.17], 1, 'Call this Overview');
  const box = "document.querySelector('.annotation-box:not(.annotation-drag)')";
  const position = () => evaluate(`['x','y','width','height'].map(n=>${box}.getAttribute(n)).join(',')`);
  const before = await position();
  const width = await evaluate(`${surface}.getBoundingClientRect().width`);
  await resize(1100, 900);
  await wait(`${surface}.getBoundingClientRect().width < ${width - 20}`);
  assert.equal(await position(), before);
  assert(await evaluate(`${pane}.inert`), 'drawing must not activate prototype controls');
  await selectScreen(1);
  await wait(`${pane}.src.endsWith('screen=2') && !!${surface}`);
  await draw([0.02, 0.01], [0.25, 0.07], 2, 'Call this Project members');
  assert.equal(requests.length, 0, 'marking must not start a run');
  await evaluate(`${button('Submit annotations')}.click()`);

  stage = 'model evidence, warning, and scoped save';
  const completed = async (count) => {
    const view = await api(`/sessions/${session}`);
    assert(view.runs.length <= count, 'unexpected run count');
    return view.runs.length === count && view.runs.every(run => run.finished_at) && view;
  };
  let view = await until(() => completed(1), 90_000);
  assert.equal(view.runs[0].result, 'succeeded', view.runs[0].error);
  assert(view.messages.some(m => /Screen 1 \(Home\): text may overlap/.test(m.content)), 'false overlap should be a named warning');
  assert.equal(requests.length, 1);
  assert.match(requests[0][0].text, /Box 1 on screen 1[^\n]+Call this Overview/);
  assert.match(requests[0][0].text, /Box 2 on screen 2[^\n]+Call this Project members/);
  const pngs = requests[0].filter(part => part.type === 'image_url');
  assert.equal(pngs.length, 2);
  for (const part of pngs) assert.match(part.image_url.url, /^data:image\/png;base64,iVBORw0KGgo/);
  const saved = await api(designRoute);
  assert.deepEqual(saved, { ...original, screens: [...replacements.map(r => r.screen), original.screens[2]] });
  await wait(`${badges}.join(',') === 'Edited,Edited,'`);

  stage = 'View, selected PNG export, and reload';
  await editor();
  await wait(`${badges}.join(',') === 'Edited,Edited,'`);
  await selectScreen(1);
  const href = await wait("document.querySelector('a[href$=\"start_screen=2\"]')?.href");
  await cdp('Page.navigate', { url: href });
  await wait("document.querySelector('[data-swift-design-state=current]')?.id==='screen-2'");
  assert.equal(await evaluate("document.querySelectorAll('[data-swift-design-frame]').length"), 3);
  const zip = await api(`${designRoute}/export.zip?screens=2`);
  assert.equal(zip.readUInt32LE(0), 0x04034b50);
  assert(zip.includes(`${id}-screen-2.png`) && !zip.includes(`${id}-screen-1.png`));

  stage = 'reopen earlier snapshot';
  await editor();
  await wait(`${badges}.join(',') === 'Edited,Edited,'`);
  await evaluate(`${button('Properties')}.click()`);
  await wait("!!document.querySelector('.history-row button')");
  await evaluate("document.querySelector('.history-row button').click()");
  await until(async () => (await api(designRoute)).screens[0].html === original.screens[0].html);
  assert.deepEqual(await api(designRoute), original);

  stage = 'unsafe edit leaves the current snapshot untouched';
  const prior = await api(`${designRoute}/snapshots/current`);
  unsafe = true;
  await api(`/sessions/${session}/messages`, 'POST', { content: 'Adjust this title.', design: id, action: 'annotate', annotations: {
    expected: { 0: original.screens[0] }, viewport: original.viewport, theme: original.theme,
    app_window: original.app_window, marks: [{ screen: 0, bounds: [100, 100, 2000, 2000], comment: 'Adjust title' }],
  } }, 204);
  view = await until(() => completed(2), 90_000);
  assert(view.runs.some(run => run.result === 'failed') && rejectedMarkup, 'unsafe markup must fail validation');
  assert.deepEqual(await api(`${designRoute}/snapshots/current`), prior);
  console.log('Screen review passed: warnings, badges, resize, one scoped edit, unsafe edit rejection, View, export, and snapshot restore.');

  if (candidateFile) {
    stage = 'Project Access dialog regression';
    const candidate = JSON.parse(readFileSync(candidateFile, 'utf8'));
    const draft = candidate.design ?? candidate;
    assert.equal(draft.screens.length, 73);
    assert.equal(draft.screens[61].name, 'Project Access · Save rejected');
    assert.equal(draft.screens[62].name, 'Project Access · Save result unconfirmed');
    mkdirSync(join(scratch, 'designs'), { recursive: true });
    writeFileSync(join(scratch, 'designs/project-access.json'), JSON.stringify(draft));
    await resize(draft.viewport.width, draft.viewport.height);
    await cdp('Page.navigate', { url: 'about:blank' });
    const source = (await api('/designs/project-access/render')).toString();
    const audit = readFileSync(join(root, 'crates/server/src/render/scripts/audit.rs'), 'utf8').split('r##"')[1].split('"##;')[0];
    const html = source.replace(/<meta http-equiv="Content-Security-Policy"[^>]*>/i, '').replace('<head>', `<head><base href="${app}/">`)
      .replace('<main class="design"', '<main class="design" data-swift-design-prototype-audit')
      .replace('</body>', () => `<script>${audit}</script></body>`);
    const frames = await cdp('Page.getFrameTree');
    await cdp('Page.setDocumentContent', { frameId: frames.frameTree.frame.id, html });
    await wait("document.documentElement.hasAttribute('data-swift-design-audited-screens')");
    const result = await evaluate(`(() => {
      const data = key => JSON.parse(document.documentElement.getAttribute('data-swift-design-' + key));
      return {measurements:data('audited-screens'),findings:data('findings'),probes:data('prototype-checks')};
    })()`);
    assert.equal(result.measurements.length, 73);
    const dialogs = result.findings.filter(f => [61, 62].includes(f.screen) && ['overlap', 'overflow', 'empty', 'off_screen', 'overfull', 'window_geometry'].includes(f.kind));
    assert.deepEqual(dialogs, []);
    assert(result.probes.some(probe => probe.screen === 62 && probe.keys > 0));
    assert.deepEqual(result.probes.filter(probe => [61, 62].includes(probe.screen) && probe.failures), []);
    console.log('Project Access copy passed: both dialogs fit, no overlap warnings, and interaction probes passed.');
  }
} catch (error) {
  console.error(`Failed during ${stage}: ${error.stack}`);
  console.error(logs.join('').split('\n').slice(-28).join('\n'));
  process.exitCode = 1;
} finally {
  socket?.close();
  for (const child of [browser, server]) {
    if (child && child.exitCode === null && child.signalCode === null) {
      const done = new Promise(resolve => child.once('exit', resolve));
      child.kill();
      await done;
    }
  }
  model.close();
  rmSync(scratch, { recursive: true, force: true });
}
