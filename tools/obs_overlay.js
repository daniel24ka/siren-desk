// Puts the alerts overlay into the live OBS scene, over obs-websocket v5 (no OBS restart, no change on air until "show").
//   node tools/obs_overlay.js status          is it there, is it shown, where it sits
//   node tools/obs_overlay.js add             create it hidden, at the top of the program scene
//   node tools/obs_overlay.js show | hide     on air / off air
//   node tools/obs_overlay.js testshot <query> <file> [shots]   a hidden twin (page?query, e.g. api=http://127.0.0.1:8152&lang=en),
//                                             photographed [shots] times 4.5 s apart (file-1.png, file-2.png...), then removed
//   node tools/obs_overlay.js lang he|en|both    the language on air (both: Hebrew and English in turn)
//   node tools/obs_overlay.js shot <file>     the program as it goes out
// The page is a local file (obs/alerts-overlay.html): a stopped SirenDesk leaves it empty, never an error page on air.

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const NAME = 'התרעות פיקוד העורף (אזעקות)';
const PAGE = path.join(__dirname, '..', 'obs', 'alerts-overlay.html');

const cfg = JSON.parse(fs.readFileSync(path.join(process.env.APPDATA, 'obs-studio', 'plugin_config', 'obs-websocket', 'config.json'), 'utf8'));
const sha = (s) => crypto.createHash('sha256').update(s).digest('base64');

function connect() {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${cfg.server_port || 4455}`);
    const pending = new Map();
    let seq = 0;
    const call = (requestType, requestData = {}) => new Promise((res, rej) => {
      const requestId = String(++seq);
      pending.set(requestId, { res, rej });
      ws.send(JSON.stringify({ op: 6, d: { requestType, requestId, requestData } }));
    });
    ws.onerror = () => reject(new Error('OBS WebSocket not reachable'));
    ws.onmessage = (ev) => {
      const m = JSON.parse(ev.data);
      if (m.op === 0) {
        const d = { rpcVersion: 1 };
        if (m.d.authentication) {
          const { challenge, salt } = m.d.authentication;
          d.authentication = sha(sha(cfg.server_password + salt) + challenge);
        }
        ws.send(JSON.stringify({ op: 1, d }));
      } else if (m.op === 2) {
        resolve({ call, close: () => ws.close() });
      } else if (m.op === 7) {
        const p = pending.get(m.d.requestId);
        pending.delete(m.d.requestId);
        if (!p) return;
        if (m.d.requestStatus.result) p.res(m.d.responseData || {});
        else p.rej(new Error(`${m.d.requestType}: ${m.d.requestStatus.comment || m.d.requestStatus.code}`));
      }
    };
  });
}

async function scene(obs) {
  return (await obs.call('GetCurrentProgramScene')).currentProgramSceneName;
}

async function item(obs, sceneName, name) {
  const { sceneItems } = await obs.call('GetSceneItemList', { sceneName });
  return sceneItems.find((i) => i.sourceName === name);
}

async function toTop(obs, sceneName, id) {
  const { sceneItems } = await obs.call('GetSceneItemList', { sceneName });
  const top = Math.max(...sceneItems.map((i) => i.sceneItemIndex));
  await obs.call('SetSceneItemIndex', { sceneName, sceneItemId: id, sceneItemIndex: top });
}

async function create(obs, sceneName, name, settings) {
  const { sceneItemId } = await obs.call('CreateInput', {
    sceneName,
    inputName: name,
    inputKind: 'browser_source',
    inputSettings: { width: 1920, height: 1080, shutdown: false, restart_when_active: false, reroute_audio: false, ...settings },
    sceneItemEnabled: false,
  });
  await obs.call('SetSceneItemTransform', { sceneName, sceneItemId, sceneItemTransform: { positionX: 0, positionY: 0, scaleX: 1, scaleY: 1 } });
  await toTop(obs, sceneName, sceneItemId);
  return sceneItemId;
}

async function shot(obs, source, file, width = 1920) {
  const r = await obs.call('GetSourceScreenshot', { sourceName: source, imageFormat: 'png', imageWidth: width });
  fs.writeFileSync(file, Buffer.from(r.imageData.split(',')[1], 'base64'));
}

async function main() {
  const [cmd = 'status', a, b] = process.argv.slice(2);
  const obs = await connect();
  try {
    const sceneName = await scene(obs);
    const it = await item(obs, sceneName, NAME);
    if (cmd === 'status') {
      const { outputActive } = await obs.call('GetStreamStatus');
      console.log(`scene ${sceneName}, live ${outputActive}, overlay ${it ? `id ${it.sceneItemId} shown=${it.sceneItemEnabled} index ${it.sceneItemIndex}` : 'not added'}`);
    } else if (cmd === 'add') {
      if (it) return console.log('already there');
      if (!fs.existsSync(PAGE)) throw new Error(`no page at ${PAGE}`);
      const id = await create(obs, sceneName, NAME, { is_local_file: true, local_file: PAGE.replace(/\\/g, '/') });
      console.log(`added hidden, id ${id}, top of "${sceneName}"`);
    } else if (cmd === 'show' || cmd === 'hide') {
      if (!it) throw new Error('not added');
      await obs.call('SetSceneItemEnabled', { sceneName, sceneItemId: it.sceneItemId, sceneItemEnabled: cmd === 'show' });
      if (cmd === 'show') await toTop(obs, sceneName, it.sceneItemId);
      console.log(`${NAME}: ${cmd === 'show' ? 'on air' : 'off air'}`);
    } else if (cmd === 'testshot') {
      // a hidden twin that reads a test copy of SirenDesk; nothing of it reaches the program
      const twin = `${NAME} - test`;
      const url = 'file:///' + PAGE.replace(/\\/g, '/') + '?' + a;
      const count = Number(process.argv[5]) || 1;
      try { await obs.call('RemoveInput', { inputName: twin }); } catch (e) { /* none yet */ }
      await create(obs, sceneName, twin, { is_local_file: false, url });
      try {
        await new Promise((r) => setTimeout(r, 5000));
        for (let k = 1; k <= count; k++) {
          const file = count === 1 ? b : b.replace(/\.png$/, `-${k}.png`);
          await shot(obs, twin, file);
          console.log(`test shot -> ${file}`);
          if (k < count) await new Promise((r) => setTimeout(r, 4500));
        }
      } finally {
        await obs.call('RemoveInput', { inputName: twin });
      }
    } else if (cmd === 'lang') {
      if (!it) throw new Error('not added');
      if (!['he', 'en', 'both'].includes(a)) throw new Error('he, en or both');
      // (a local file takes no query: the page is then read as a file:// address that carries one)
      const settings = a === 'he'
        ? { is_local_file: true, local_file: PAGE.replace(/\\/g, '/') }
        : { is_local_file: false, url: 'file:///' + PAGE.replace(/\\/g, '/') + '?lang=' + a };
      await obs.call('SetInputSettings', { inputName: NAME, inputSettings: settings });
      console.log(`${NAME}: language ${a}`);
    } else if (cmd === 'shot') {
      await shot(obs, sceneName, a);
      console.log(`program -> ${a}`);
    }
  } finally {
    obs.close();
  }
}

main().catch((e) => { console.error(e.message); process.exit(1); });
