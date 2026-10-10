import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import QRCode from 'qrcode';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const endpoint = 'https://ilinkai.weixin.qq.com/';
const baseInfo = { channel_version: '2.4.9', bot_agent: 'k-Coder/0.10.0 (weixin-smoke)' };
const maxBody = 1024 * 1024;
const challenge = `kcoder-${crypto.randomBytes(6).toString('hex')}`;
const pageKey = crypto.randomBytes(24).toString('hex');
let stopped = false;
let verification = '';
let origin;
let server;
let activeChild;
let restartRequested = false;
let ui = { phase: '准备中', qr: '', challenge, verified: false, canRestart: false, qrStatus: '' };
const controllers = new Set();
const qrStatuses = new Set(['wait', 'scaned', 'confirmed', 'expired', 'scaned_but_redirect',
  'need_verifycode', 'verify_code_blocked', 'binded_redirect']);

function officialBase(value) {
  const url = new URL(value);
  assert(url.protocol === 'https:' && /^ilink[a-z0-9-]*\.weixin\.qq\.com$/.test(url.hostname)
    && !url.username && !url.password && !url.port && url.pathname === '/'
    && !url.search && !url.hash, 'untrusted API host');
  return url.href;
}

function messageForChallenge(message, owner, expected, seen) {
  if (message.message_type !== 1 || message.from_user_id !== owner || message.group_id
    || !message.context_token || !Array.isArray(message.item_list)) return null;
  const text = message.item_list.filter(item => item.type === 1)
    .map(item => item.text_item?.text ?? '').join('\n').trim();
  if (text !== expected || message.item_list.some(item => item.type !== 1)) return null;
  const id = String(message.message_id ?? message.msg_id ?? message.client_id ?? '');
  if (!id || seen.has(id)) return null;
  seen.add(id);
  return { input: text, context: message.context_token };
}

function update(phase, extra = {}) {
  ui = { ...ui, phase, ...extra };
  console.log(JSON.stringify({ phase, verified: ui.verified, qrStatus: ui.qrStatus,
    time: new Date().toISOString() }));
}

function restartLogin() {
  if (stopped || !ui.canRestart || restartRequested) return false;
  restartRequested = true;
  verification = '';
  update('正在重新生成二维码', { qr: '', needCode: false, canRestart: false, result: '', qrStatus: '' });
  for (const controller of controllers) controller.abort();
  return true;
}

function escapeHtml(value) {
  return String(value ?? '').replace(/[&<>"']/g,
    character => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]);
}

async function request(base, route, body, token, timeout = 35000) {
  const controller = new AbortController();
  controllers.add(controller);
  const timer = setTimeout(() => controller.abort(), timeout);
  const headers = { 'iLink-App-Id': 'bot', 'iLink-App-ClientVersion': '132105' };
  if (body !== undefined) {
    Object.assign(headers, { 'Content-Type': 'application/json', AuthorizationType: 'ilink_bot_token',
      'X-WECHAT-UIN': Buffer.from(String(crypto.randomBytes(4).readUInt32BE())).toString('base64') });
  }
  if (token) headers.Authorization = `Bearer ${token}`;
  try {
    const response = await fetch(new URL(route, officialBase(base)), {
      method: body === undefined ? 'GET' : 'POST', headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: controller.signal, redirect: 'error',
    });
    if (!response.ok) throw new Error(`微信接口 HTTP ${response.status}`);
    const chunks = [];
    let size = 0;
    for await (const chunk of response.body) {
      size += chunk.length;
      if (size > maxBody) { controller.abort(); throw new Error('微信接口响应超过上限'); }
      chunks.push(Buffer.from(chunk));
    }
    const result = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if ((result.ret !== undefined && result.ret !== 0) || (result.errcode && result.errcode !== 0))
      throw new Error(`微信接口返回错误 ${Number(result.ret || result.errcode)}`);
    return result;
  } finally { clearTimeout(timer); controllers.delete(controller); }
}

function stop() {
  stopped = true;
  for (const controller of controllers) controller.abort();
  activeChild?.kill();
  server?.close();
  server?.closeAllConnections();
}

async function runRuntime(input, binary) {
  assert(/^kcoder-[a-f0-9]{12}$/.test(input));
  return new Promise((resolve, reject) => {
    activeChild = spawn(binary, ['--ignored', '--exact', 'live_weixin_runtime_once', '--nocapture'],
      { cwd: root, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    let output = '';
    let size = 0;
    const timer = setTimeout(() => activeChild?.kill(), 60000);
    for (const stream of [activeChild.stdout, activeChild.stderr]) stream.on('data', chunk => {
      size += chunk.length;
      if (size > maxBody) { activeChild?.kill(); return; }
      output += chunk.toString();
    });
    activeChild.on('error', () => { clearTimeout(timer); reject(new Error('无法启动验证运行时')); });
    activeChild.on('close', code => {
      clearTimeout(timer);
      activeChild = undefined;
      const line = output.split(/\r?\n/).find(item => item.startsWith('KC_WEIXIN_SMOKE_RESULT '));
      if (code !== 0 || !line || size > maxBody) { reject(new Error('k-Coder 运行时验证失败')); return; }
      try { resolve(JSON.parse(line.slice('KC_WEIXIN_SMOKE_RESULT '.length))); }
      catch { reject(new Error('验证运行时返回格式错误')); }
    });
    activeChild.stdin.end(JSON.stringify({ input }));
  });
}

function page() {
  return `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>微信最小验证</title>
<style>body{font:16px system-ui;margin:24px;background:#f5f6f7;color:#222}main{max-width:620px;margin:auto;background:white;padding:24px;border-radius:16px}img{display:block;width:min(300px,100%);height:auto;margin:20px auto}[hidden]{display:none!important}code{background:#eef5f0;padding:8px;display:inline-block;font-size:20px}button{padding:8px 16px;margin:4px}p{line-height:1.6}small{color:#666}input{padding:8px;font-size:18px}</style>
<main><h1>微信 → k-Coder 最小验证</h1><p id="phase">${escapeHtml(ui.phase)}</p>
<img id="qr" alt="微信扫码连接二维码" ${ui.qr ? `src="${escapeHtml(ui.qr)}"` : 'hidden'}>
<p><small id="status">${escapeHtml(ui.qrStatus)}</small></p>
<p>先扫码并在手机确认；等页面显示“已连接”后，在 Bot 会话发送：</p>
<code id="challenge">${escapeHtml(challenge)}</code>
<p><small>只接收此测试口令，其他消息全部忽略。只运行隔离目录中的一次只读检查。Bot 凭据只存内存；这是测试运行时，不是桌面完整功能。</small></p>
<form id="verify" hidden><label>输入手机显示的数字：<input id="number" inputmode="numeric" maxlength="12" autocomplete="off"></label><button>确认</button></form>
<p id="result">${escapeHtml(ui.result)}</p><button id="restart" ${ui.canRestart ? '' : 'disabled'}>重新生成二维码</button><button id="stop">停止验证</button></main>
<script>
const prefix=location.pathname;
async function poll(){
  try{
    const response=await fetch(prefix+'/state',{cache:'no-store'});
    if(!response.ok)throw new Error();
    const s=await response.json();
    document.getElementById('phase').textContent=s.phase;
    document.getElementById('challenge').textContent=s.challenge;
    const image=document.getElementById('qr');
    image.hidden=!s.qr;
    if(s.qr&&image.getAttribute('src')!==s.qr)image.src=s.qr;
    if(!s.qr)image.removeAttribute('src');
    document.getElementById('status').textContent=s.qrStatus?'扫码状态：'+s.qrStatus:'';
    document.getElementById('verify').hidden=!s.needCode;
    document.getElementById('restart').disabled=!s.canRestart;
    document.getElementById('result').textContent=s.result||'';
  }catch{document.getElementById('phase').textContent='验证进程已停止，需要重新启动验证工具'}
}
document.getElementById('verify').onsubmit=async e=>{
  e.preventDefault();
  const response=await fetch(prefix+'/verify',{method:'POST',body:JSON.stringify({code:document.getElementById('number').value})});
  if(response.ok)document.getElementById('number').value='';
};
document.getElementById('restart').onclick=async()=>{
  document.getElementById('restart').disabled=true;
  await fetch(prefix+'/restart',{method:'POST'});
  await poll();
};
document.getElementById('stop').onclick=()=>fetch(prefix+'/stop',{method:'POST'});
poll();setInterval(poll,1000);
</script></html>`;
}

async function startPage() {
  server = http.createServer(async (req, res) => {
    const prefix = `/session/${pageKey}`;
    res.setHeader('Cache-Control', 'no-store');
    res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Content-Security-Policy', "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'");
    if (req.headers.host !== new URL(origin).host || !req.url?.startsWith(prefix)
      || (req.method === 'POST' && req.headers.origin !== origin)) { res.writeHead(403).end(); return; }
    if (req.method === 'GET' && req.url === prefix) {
      res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(page()); return;
    }
    if (req.method === 'GET' && req.url === `${prefix}/state`) {
      res.setHeader('Content-Type', 'application/json; charset=utf-8'); res.end(JSON.stringify(ui)); return;
    }
    if (req.method === 'POST' && req.url === `${prefix}/restart`) {
      res.writeHead(restartLogin() ? 200 : 409, { 'Content-Type': 'application/json' }).end('{}'); return;
    }
    if (req.method === 'POST' && req.url === `${prefix}/stop`) { res.end('{}'); stop(); return; }
    if (req.method === 'POST' && req.url === `${prefix}/verify`) {
      try {
        let body = ''; for await (const chunk of req) { body += chunk; if (body.length > 256) throw new Error(); }
        const code = JSON.parse(body).code;
        if (!ui.needCode || !/^\d{1,12}$/.test(code)) throw new Error();
        verification = code; update('正在校验手机验证码', { needCode: false }); res.end('{}');
      } catch { res.writeHead(400).end('{}'); }
      return;
    }
    res.writeHead(404).end();
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
  console.log(JSON.stringify({ previewUrl: `${origin}/session/${pageKey}` }));
  setTimeout(stop, 25 * 60000).unref();
}

const pause = () => new Promise(resolve => setTimeout(resolve, 1000));

async function login({ api = request, wait = pause, now = Date.now } = {}) {
  let qr;
  let base = endpoint;
  let deadline = now() + 10 * 60000;
  while (!stopped && now() < deadline) {
    if (restartRequested) {
      restartRequested = false;
      qr = undefined;
      deadline = now() + 10 * 60000;
    }
    if (!qr) {
      qr = await api(endpoint, 'ilink/bot/get_bot_qrcode?bot_type=3', { local_token_list: [] }, undefined, 20000);
      assert(typeof qr.qrcode === 'string' && qr.qrcode && typeof qr.qrcode_img_content === 'string' && qr.qrcode_img_content);
      base = endpoint;
      verification = '';
      update('请用手机微信扫码，并在手机确认连接', {
        qr: await QRCode.toDataURL(qr.qrcode_img_content, { width: 360 }),
        needCode: false, canRestart: true, result: '', qrStatus: '',
      });
    }
    let status;
    try {
      const route = `ilink/bot/get_qrcode_status?qrcode=${encodeURIComponent(qr.qrcode)}`
        + (verification ? `&verify_code=${encodeURIComponent(verification)}` : '');
      status = await api(base, route);
    } catch (error) {
      if (restartRequested || stopped) continue;
      if (!['AbortError', 'TimeoutError', 'TypeError'].includes(error.name)) throw error;
      if (ui.phase !== '正在等待微信扫码状态；网络超时会自动重试')
        update('正在等待微信扫码状态；网络超时会自动重试');
      await wait();
      continue;
    }
    if (restartRequested || stopped) continue;
    if (!qrStatuses.has(status.status)) throw new Error('微信接口返回未支持的扫码状态');
    if (ui.qrStatus !== status.status) {
      update(ui.phase, { qrStatus: status.status });
    }
    if (status.status === 'confirmed') {
      assert(typeof status.bot_token === 'string' && status.bot_token && typeof status.ilink_user_id === 'string' && status.ilink_user_id);
      update('微信已确认绑定', { qr: '', needCode: false, canRestart: false });
      return { token: status.bot_token, owner: status.ilink_user_id, base: officialBase(status.baseurl || base) };
    }
    if (status.status === 'wait' && ui.phase !== '请用手机微信扫码，并在手机确认连接')
      update('请用手机微信扫码，并在手机确认连接');
    if (status.status === 'scaned_but_redirect') {
      base = officialBase(`https://${status.redirect_host}/`);
      update('已扫码，正在继续确认绑定');
    }
    if (status.status === 'scaned') {
      verification = '';
      if (ui.phase !== '已扫码，请在手机上确认') update('已扫码，请在手机上确认', { needCode: false });
    }
    if (status.status === 'expired') {
      update('二维码已过期，请点击“重新生成二维码”', { qr: '', needCode: false, canRestart: true });
      while (!stopped && !restartRequested && now() < deadline) await wait();
      continue;
    }
    if (status.status === 'binded_redirect') throw new Error('此 Bot 已绑定，验证工具没有旧凭据；请在手机选择新的连接');
    if (status.status === 'verify_code_blocked') throw new Error('验证码错误次数过多，请稍后重试');
    if (status.status === 'need_verifycode') {
      const incorrect = Boolean(verification);
      verification = '';
      update(incorrect ? '数字不匹配，请重新输入手机显示的数字' : '请在本页面输入手机显示的数字', { needCode: true });
      while (!stopped && !restartRequested && !verification && now() < deadline) await wait();
      continue;
    }
    await wait();
  }
  throw new Error(stopped ? '已停止验证' : '扫码超时');
}

async function verifyOnce(binary) {
  const account = await login();
  update('已连接；请在微信 Bot 会话发送测试口令', { qr: '', needCode: false, canRestart: false });
  let cursor = '';
  const seen = new Set();
  const deadline = Date.now() + 10 * 60000;
  while (!stopped && Date.now() < deadline) {
    let result;
    try { result = await request(account.base, 'ilink/bot/getupdates', { get_updates_buf: cursor, base_info: baseInfo }, account.token); }
    catch (error) { if (error.name === 'AbortError' || error.name === 'TimeoutError') continue; throw error; }
    cursor = result.get_updates_buf ?? cursor;
    for (const message of result.msgs ?? []) {
      const match = messageForChallenge(message, account.owner, challenge, seen);
      if (!match) continue;
      update('已收到口令，正在运行隔离的 k-Coder 检查');
      const runtime = await runRuntime(match.input, binary);
      assert(runtime.state === 'completed' && runtime.tools === 1 && runtime.providerCalls === 2);
      await request(account.base, 'ilink/bot/sendmessage', { base_info: baseInfo, msg: {
        from_user_id: '', to_user_id: account.owner, client_id: `kcoder-smoke-${crypto.randomUUID()}`,
        message_type: 2, message_state: 2, item_list: [{ type: 1, text_item: { text: runtime.reply } }],
        context_token: match.context,
      } }, account.token, 15000);
      update('微信接口已接受回复，请确认手机是否收到', { result: runtime.reply, verified: true });
      return;
    }
    await pause();
  }
  throw new Error(stopped ? '已停止验证' : '等待测试口令超时');
}

async function main(binary) {
  assert(binary && path.isAbsolute(binary) && fs.statSync(binary).isFile(), '请提供编译后的 weixin_smoke_runtime 测试程序绝对路径');
  await startPage();
  process.once('SIGINT', stop); process.once('SIGTERM', stop);
  while (!stopped) {
    restartRequested = false;
    process.exitCode = 0;
    try {
      await verifyOnce(binary);
      return;
    } catch (error) {
      if (restartRequested && !stopped) continue;
      update('验证未完成，可以重新生成二维码', {
        qr: '', needCode: false, canRestart: !stopped,
        result: /^微信接口|^扫码|^等待|^验证码|^此 Bot|^已停止|^二维码|^k-Coder/.test(error.message)
          ? error.message : '网络、协议或本地运行时校验失败；未输出凭据或接口原文',
      });
      process.exitCode = 1;
      while (!stopped && !restartRequested) await pause();
    }
  }
}

if (process.argv.includes('--self-test')) {
  test('API host validation rejects credential leaks and redirects', () => {
    assert.equal(officialBase(endpoint), endpoint);
    for (const url of ['http://ilinkai.weixin.qq.com/', 'https://ilinkai.weixin.qq.com.attacker.test/',
      'https://127.0.0.1/', 'https://user:secret@ilinkai.weixin.qq.com/', 'https://ilinkai.weixin.qq.com:8443/',
      'https://ilinkai.weixin.qq.com/path', 'https://ilinkai.weixin.qq.com/?token=secret']) assert.throws(() => officialBase(url));
  });
  test('only bound owner exact text challenge is accepted once', () => {
    const message = { message_type: 1, from_user_id: 'owner', context_token: 'fake', message_id: '1', item_list: [{ type: 1, text_item: { text: challenge } }] };
    const seen = new Set();
    for (const change of [{ from_user_id: 'other' }, { group_id: 'group' }, { message_type: 2 }, { context_token: '' },
      { item_list: [{ type: 1, text_item: { text: 'delete project' } }] }, { item_list: [{ type: 2 }] }])
      assert.equal(messageForChallenge({ ...message, ...change }, 'owner', challenge, seen), null);
    assert.equal(messageForChallenge(message, 'owner', challenge, seen).input, challenge);
    assert.equal(messageForChallenge(message, 'owner', challenge, seen), null);
  });
  test('QR expiry waits for manual retry, then binds after redirect and verification', async () => {
    const saved = { ui, verification, restartRequested, stopped };
    const phases = ['wait', 'expired', 'scaned_but_redirect', 'need_verifycode', 'scaned', 'confirmed'];
    let generated = 0;
    let checkedCode = false;
    try {
      stopped = false;
      restartRequested = false;
      verification = '';
      ui = { phase: '准备中', qr: '', challenge, verified: false, canRestart: false, qrStatus: '' };
      const account = await login({
        now: () => 0,
        wait: async () => {
          if (ui.qrStatus === 'expired') {
            assert.equal(ui.qr, '');
            assert.equal(generated, 1);
            assert.equal(restartLogin(), true);
            assert.equal(restartLogin(), false);
          }
          if (ui.needCode) verification = '1234';
        },
        api: async (base, route, body) => {
          if (route.startsWith('ilink/bot/get_bot_qrcode')) {
            assert.equal(base, endpoint);
            assert.deepEqual(body, { local_token_list: [] });
            generated++;
            return { qrcode: `fake-${generated}`, qrcode_img_content: 'https://example.invalid/test' };
          }
          const status = phases.shift();
          assert(status);
          assert.match(ui.qr, /^data:image\/png;base64,/);
          if (['need_verifycode', 'scaned', 'confirmed'].includes(status))
            assert.equal(base, 'https://ilink-test.weixin.qq.com/');
          if (status === 'scaned') {
            assert.match(route, /verify_code=1234/);
            checkedCode = true;
          }
          if (status === 'confirmed') {
            assert(!route.includes('verify_code='));
            return { status, bot_token: 'test-token', ilink_user_id: 'test-user', baseurl: endpoint };
          }
          return { status, redirect_host: 'ilink-test.weixin.qq.com' };
        },
      });
      assert.equal(generated, 2);
      assert(checkedCode);
      assert.equal(account.owner, 'test-user');
      assert.equal(ui.qr, '');
      assert.equal(ui.canRestart, false);
      assert.equal(restartLogin(), false);
    } finally { ({ ui, verification, restartRequested, stopped } = saved); }
  });
  test('unknown QR states fail closed and page hides absent QR even without scripts', async () => {
    const saved = { ui, verification, restartRequested, stopped };
    try {
      stopped = false;
      restartRequested = false;
      ui = { phase: '<script>bad</script>', qr: '', challenge, canRestart: true, result: '<img onerror=bad>' };
      const markup = page();
      assert.match(markup, /\[hidden\]\{display:none!important\}/);
      assert.match(markup, /id="qr"[^>]*hidden/);
      assert(!markup.includes('<script>bad</script>'));
      assert(!markup.includes('<img onerror=bad>'));
      assert.match(markup, /id="restart"/);
      await assert.rejects(login({
        now: () => 0,
        wait: async () => {},
        api: async (_base, route) => route.includes('get_bot_qrcode')
          ? { qrcode: 'fake', qrcode_img_content: 'https://example.invalid/test' }
          : { status: 'unknown-state', bot_token: 'should-not-be-used' },
      }), /未支持的扫码状态/);
    } finally { ({ ui, verification, restartRequested, stopped } = saved); }
  });
} else {
  main(process.argv[2]).catch(error => {
    update('验证启动失败', { qr: '', needCode: false, canRestart: false, result: '请检查验证程序路径和本地端口；未输出凭据或接口原文' });
    process.exitCode = 1;
    for (const controller of controllers) controller.abort();
    activeChild?.kill();
  });
}
