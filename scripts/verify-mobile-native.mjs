#!/usr/bin/env node
// 手机局域网直连（P10-188）原生验收工具。
//
// 这个脚本不依赖任何第三方包：它用 `node:tls` 与手写的 WebSocket 帧编解码
// 扮演手机端，直连「真实运行中的桌面客户端」启动的网关，因此可以验证
// Playwright 打桩与 Rust 集成测试都覆盖不到的那一段——真实进程、真实网卡、
// 真实自签证书、真实 SQLite 会话数据。
//
// 用法：
//   node scripts/verify-mobile-native.mjs seed      # 备份并写入验收用状态
//   node scripts/verify-mobile-native.mjs phone     # 以手机身份跑完整链路
//   node scripts/verify-mobile-native.mjs restore   # 还原状态，移除验收设备
//
// 环境变量：
//   KCODER_DATA_ROOT  覆盖应用数据目录（默认 %APPDATA%\com.kcoder.app\runtime-data）
//   KCODER_LAN_IP     覆盖监听地址（默认自动探测）

import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { connect as tlsConnect } from "node:tls";

const ROAMING =
  process.env.APPDATA || path.join(os.homedir(), "AppData", "Roaming");
const DATA_ROOT =
  process.env.KCODER_DATA_ROOT ||
  path.join(ROAMING, "com.kcoder.app", "runtime-data");
const MOBILE_DIR = path.join(DATA_ROOT, "mobile");
const STATE_PATH = path.join(MOBILE_DIR, "state.json");
const BACKUP_PATH = path.join(MOBILE_DIR, "state.json.verify-backup");
const CERT_PATH = path.join(MOBILE_DIR, "tls", "cert.pem");

// 验收专用设备。密钥是固定值，只用于本机回环/局域网自验，
// 因此 `restore` 必须把这条记录删掉，绝不能留在用户环境里。
const DEVICE_ID = "00000000-0000-4000-8000-0000000000a1";
const DEVICE_SECRET = "native-verify-device-secret";
const REFRESH_TOKEN = "native-verify-refresh-token";
const REVOKED_DEVICE_ID = "00000000-0000-4000-8000-0000000000a2";
const REVOKED_DEVICE_SECRET = "native-verify-revoked-secret";

const PORT = Number(process.env.KCODER_MOBILE_PORT || 8787);
const HOST = process.env.KCODER_LAN_IP || detectLanIp();

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function detectLanIp() {
  const candidates = [];
  for (const entries of Object.values(os.networkInterfaces())) {
    for (const entry of entries ?? []) {
      if (entry.family !== "IPv4" || entry.internal) continue;
      if (entry.address.startsWith("169.254.")) continue;
      candidates.push(entry.address);
    }
  }
  if (candidates.length === 0) {
    throw new Error("未找到局域网 IPv4 地址，请用 KCODER_LAN_IP 显式指定");
  }
  return candidates[0];
}

const sha256Hex = (buffer) => createHash("sha256").update(buffer).digest("hex");

const sha256Fingerprint = (der) =>
  createHash("sha256")
    .update(der)
    .digest()
    .toString("hex")
    .toUpperCase()
    .match(/../g)
    .join(":");

function pemToDer(pem) {
  const body = pem
    .replace(/-----BEGIN [^-]+-----/, "")
    .replace(/-----END [^-]+-----/, "")
    .replace(/\s+/g, "");
  return Buffer.from(body, "base64");
}

// ---------------------------------------------------------------------------
// 状态文件读写
// ---------------------------------------------------------------------------

function seed() {
  fs.mkdirSync(MOBILE_DIR, { recursive: true });
  if (fs.existsSync(STATE_PATH) && !fs.existsSync(BACKUP_PATH)) {
    fs.copyFileSync(STATE_PATH, BACKUP_PATH);
    console.log(`已备份原状态 → ${BACKUP_PATH}`);
  }
  const now = Date.now();
  const state = {
    schemaVersion: 1,
    settings: { enabled: true, bindAddress: HOST, port: PORT },
    devices: [
      {
        id: DEVICE_ID,
        name: "原生验收设备",
        platform: "verify-harness",
        createdAtMs: now,
        lastSeenAtMs: now,
        revoked: false,
        secretHash: sha256Hex(Buffer.from(DEVICE_SECRET, "utf8")),
        refreshHash: sha256Hex(Buffer.from(REFRESH_TOKEN, "utf8")),
      },
      {
        id: REVOKED_DEVICE_ID,
        name: "原生验收已撤销设备",
        platform: "verify-harness",
        createdAtMs: now,
        lastSeenAtMs: now,
        revoked: true,
        secretHash: sha256Hex(Buffer.from(REVOKED_DEVICE_SECRET, "utf8")),
        refreshHash: sha256Hex(Buffer.from(REFRESH_TOKEN, "utf8")),
      },
    ],
  };
  fs.writeFileSync(STATE_PATH, `${JSON.stringify(state, null, 2)}\n`, "utf8");
  console.log(`已写入验收状态 → ${STATE_PATH}`);
  console.log(`  监听地址 ${HOST}:${PORT}（enabled=true）`);
  console.log(`  验收设备 ${DEVICE_ID}`);
  console.log(`  已撤销设备 ${REVOKED_DEVICE_ID}`);
}

function restore() {
  let restored = "已删除";
  if (fs.existsSync(BACKUP_PATH)) {
    fs.renameSync(BACKUP_PATH, STATE_PATH);
    restored = "已还原备份";
  } else if (fs.existsSync(STATE_PATH)) {
    fs.rmSync(STATE_PATH);
  }
  console.log(`状态文件${restored}：${STATE_PATH}`);
  const tlsDir = path.join(MOBILE_DIR, "tls");
  if (fs.existsSync(tlsDir)) {
    console.log(`保留自签证书目录（指纹在重启后保持稳定，属于设计行为）：${tlsDir}`);
  }
  console.log("验收设备记录已移除。");
}

// ---------------------------------------------------------------------------
// 极简 socket 读取器
// ---------------------------------------------------------------------------

class Reader {
  constructor(socket) {
    this.socket = socket;
    this.buffer = Buffer.alloc(0);
    this.closed = false;
    this.failure = null;
    socket.on("data", (chunk) => {
      this.buffer = Buffer.concat([this.buffer, chunk]);
    });
    socket.on("close", () => {
      this.closed = true;
    });
    socket.on("error", (error) => {
      this.failure = error;
    });
  }

  async waitFor(predicate, timeoutMs, label) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const value = predicate();
      if (value !== undefined && value !== null && value !== false) return value;
      if (this.failure) throw this.failure;
      if (this.closed) throw new Error(`连接已关闭，等待「${label}」失败`);
      if (Date.now() > deadline) throw new Error(`等待「${label}」超时`);
      await sleep(20);
    }
  }

  async readHttpResponse(timeoutMs = 5000, label = "HTTP 响应") {
    const headEnd = await this.waitFor(() => {
      const index = this.buffer.indexOf("\r\n\r\n");
      return index >= 0 ? index : null;
    }, timeoutMs, label);
    const head = this.buffer.subarray(0, headEnd).toString("latin1");
    this.buffer = this.buffer.subarray(headEnd + 4);
    const [statusLine, ...headerLines] = head.split("\r\n");
    const status = Number(statusLine.split(" ")[1]);
    const headers = {};
    for (const line of headerLines) {
      const index = line.indexOf(":");
      if (index < 0) continue;
      headers[line.slice(0, index).trim().toLowerCase()] = line
        .slice(index + 1)
        .trim();
    }
    let body = Buffer.alloc(0);
    const length = Number(headers["content-length"] ?? 0);
    if (length > 0) {
      await this.waitFor(
        () => (this.buffer.length >= length ? true : null),
        timeoutMs,
        `${label} 正文`,
      );
      body = this.buffer.subarray(0, length);
      this.buffer = this.buffer.subarray(length);
    }
    return { status, headers, body, statusLine };
  }
}

function encodeFrame(opcode, payload) {
  const data = Buffer.isBuffer(payload) ? payload : Buffer.from(payload, "utf8");
  const mask = randomBytes(4);
  let header;
  if (data.length < 126) {
    header = Buffer.alloc(2);
    header[1] = 0x80 | data.length;
  } else if (data.length < 65536) {
    header = Buffer.alloc(4);
    header[1] = 0x80 | 126;
    header.writeUInt16BE(data.length, 2);
  } else {
    header = Buffer.alloc(10);
    header[1] = 0x80 | 127;
    header.writeBigUInt64BE(BigInt(data.length), 2);
  }
  header[0] = 0x80 | opcode;
  const masked = Buffer.alloc(data.length);
  for (let index = 0; index < data.length; index += 1) {
    masked[index] = data[index] ^ mask[index % 4];
  }
  return Buffer.concat([header, mask, masked]);
}

function tryReadFrame(reader) {
  const buffer = reader.buffer;
  if (buffer.length < 2) return null;
  const opcode = buffer[0] & 0x0f;
  const masked = (buffer[1] & 0x80) !== 0;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < 4) return null;
    length = buffer.readUInt16BE(2);
    offset = 4;
  } else if (length === 127) {
    if (buffer.length < 10) return null;
    length = Number(buffer.readBigUInt64BE(2));
    offset = 10;
  }
  let mask = null;
  if (masked) {
    if (buffer.length < offset + 4) return null;
    mask = buffer.subarray(offset, offset + 4);
    offset += 4;
  }
  if (buffer.length < offset + length) return null;
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (mask) {
    for (let index = 0; index < payload.length; index += 1) {
      payload[index] ^= mask[index % 4];
    }
  }
  reader.buffer = buffer.subarray(offset + length);
  return { opcode, payload };
}

// ---------------------------------------------------------------------------
// 传输层：真实 TLS + 手写 WebSocket
// ---------------------------------------------------------------------------

function openTls(host, port) {
  return new Promise((resolve, reject) => {
    const socket = tlsConnect(
      { host, port, rejectUnauthorized: false, servername: host },
      () => resolve(socket),
    );
    socket.once("error", reject);
    socket.setTimeout(10000, () => {
      socket.destroy(new Error("TLS 连接超时"));
    });
  });
}

async function tlsFingerprintCheck(socket) {
  const peer = socket.getPeerCertificate(true);
  if (!peer || !peer.raw) {
    return { ok: false, detail: "服务端未提供证书" };
  }
  const peerFingerprint = sha256Fingerprint(peer.raw);
  if (!fs.existsSync(CERT_PATH)) {
    return { ok: false, detail: `本地缺少 ${CERT_PATH}` };
  }
  const localFingerprint = sha256Fingerprint(
    pemToDer(fs.readFileSync(CERT_PATH, "utf8")),
  );
  return {
    ok: peerFingerprint === localFingerprint,
    detail: peerFingerprint,
    localFingerprint,
    authorized: socket.authorized,
  };
}

async function httpGet(reader, socket, requestPath, extraHeaders = {}) {
  const headers = {
    Host: `${HOST}:${PORT}`,
    Connection: "keep-alive",
    ...extraHeaders,
  };
  const lines = [
    `GET ${requestPath} HTTP/1.1`,
    ...Object.entries(headers).map(([key, value]) => `${key}: ${value}`),
    "",
    "",
  ];
  socket.write(lines.join("\r\n"));
  return reader.readHttpResponse(5000, `GET ${requestPath}`);
}

async function wsUpgrade(reader, socket) {
  const key = randomBytes(16).toString("base64");
  const lines = [
    "GET /ws HTTP/1.1",
    `Host: ${HOST}:${PORT}`,
    "Upgrade: websocket",
    "Connection: Upgrade",
    `Sec-WebSocket-Key: ${key}`,
    "Sec-WebSocket-Version: 13",
    "",
    "",
  ];
  socket.write(lines.join("\r\n"));
  const response = await reader.readHttpResponse(5000, "WebSocket 升级");
  if (response.status !== 101) {
    throw new Error(`WebSocket 升级失败：${response.statusLine}`);
  }
  const expected = createHash("sha1")
    .update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
    .digest("base64");
  const accept = response.headers["sec-websocket-accept"];
  if (accept !== expected) {
    throw new Error("Sec-WebSocket-Accept 校验失败");
  }
  return true;
}

class RpcClient {
  constructor(socket, reader) {
    this.socket = socket;
    this.reader = reader;
    this.nextId = 1;
    this.pending = new Map();
    this.notifications = [];
    this.looping = true;
    this.loopPromise = this.loop();
  }

  async loop() {
    while (this.looping) {
      const frame = tryReadFrame(this.reader);
      if (!frame) {
        if (this.reader.closed) return;
        await sleep(10);
        continue;
      }
      if (frame.opcode === 0x8) {
        this.looping = false;
        return;
      }
      if (frame.opcode === 0x9) {
        this.socket.write(encodeFrame(0xa, frame.payload));
        continue;
      }
      if (frame.opcode !== 0x1) continue;
      let message;
      try {
        message = JSON.parse(frame.payload.toString("utf8"));
      } catch {
        continue;
      }
      if (message.id !== undefined && this.pending.has(message.id)) {
        const resolve = this.pending.get(message.id);
        this.pending.delete(message.id);
        resolve(message);
      } else if (message.method) {
        this.notifications.push(message);
      }
    }
  }

  call(method, params = {}, timeoutMs = 8000) {
    const id = this.nextId;
    this.nextId += 1;
    const promise = new Promise((resolve, reject) => {
      this.pending.set(id, resolve);
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`RPC ${method} 超时`));
      }, timeoutMs);
    });
    this.socket.write(
      encodeFrame(0x1, JSON.stringify({ jsonrpc: "2.0", id, method, params })),
    );
    return promise;
  }

  close() {
    this.looping = false;
    try {
      this.socket.write(encodeFrame(0x8, Buffer.alloc(0)));
    } catch {
      // 连接可能已经断开，忽略。
    }
    this.socket.destroy();
  }
}

// ---------------------------------------------------------------------------
// 手机端链路
// ---------------------------------------------------------------------------

const results = [];

function record(name, ok, detail) {
  results.push({ name, ok, detail });
  const mark = ok ? "PASS" : "FAIL";
  console.log(`  [${mark}] ${name}${detail ? ` — ${detail}` : ""}`);
}

async function phone() {
  console.log(`目标：wss://${HOST}:${PORT}  （数据目录 ${DATA_ROOT}）`);
  console.log("");

  // --- 1. 传输层 -----------------------------------------------------------
  console.log("传输层");
  let socket;
  try {
    socket = await openTls(HOST, PORT);
    record("TCP + TLS 握手成功", true, socket.getProtocol());
  } catch (error) {
    record("TCP + TLS 握手成功", false, error.message);
    printSummary();
    process.exitCode = 1;
    return;
  }
  const reader = new Reader(socket);

  const fingerprint = await tlsFingerprintCheck(socket);
  record(
    "服务端证书指纹与本地 cert.pem 逐位一致",
    fingerprint.ok,
    fingerprint.detail,
  );
  record(
    "自签证书未被系统信任（预期行为，手机端靠指纹核对）",
    fingerprint.authorized === false,
    `authorized=${fingerprint.authorized}`,
  );

  const health = await httpGet(reader, socket, "/health");
  let healthBody = {};
  try {
    healthBody = JSON.parse(health.body.toString("utf8"));
  } catch {
    // 保持空对象，下面的断言会失败。
  }
  record(
    "GET /health 返回协议版本 1",
    health.status === 200 && healthBody.protocolVersion === 1,
    `status=${health.status} protocolVersion=${healthBody.protocolVersion} tls=${healthBody.tls}`,
  );

  const page = await httpGet(reader, socket, "/m/");
  const html = page.body.toString("utf8");
  record(
    "GET /m/ 返回内联单文件 PWA（无外部资源引用）",
    page.status === 200 &&
      html.includes("<html") &&
      !/(src|href)=["']https?:/i.test(html),
    `${html.length} 字节`,
  );

  const rootRedirect = await httpGet(reader, socket, "/");
  record(
    "GET / 重定向到 /m",
    rootRedirect.status === 308 && rootRedirect.headers.location === "/m",
    `status=${rootRedirect.status} location=${rootRedirect.headers.location}`,
  );

  // --- 2. 来源与头校验（用独立连接，避免污染主连接）------------------------
  console.log("");
  console.log("来源与请求头校验");
  for (const [label, headers, expectedStatus] of [
    ["伪造 Host 头被拒绝", { Host: "evil.example.com" }, 403],
    ["X-Forwarded-For 被拒绝", { "X-Forwarded-For": "1.2.3.4" }, 400],
    ["Forwarded 头被拒绝", { Forwarded: "for=1.2.3.4" }, 400],
  ]) {
    const probeSocket = await openTls(HOST, PORT);
    const probeReader = new Reader(probeSocket);
    const probe = await httpGet(probeReader, probeSocket, "/health", headers);
    record(label, probe.status === expectedStatus, `status=${probe.status}`);
    probeSocket.destroy();
  }

  // --- 3. JSON-RPC 生命周期 ------------------------------------------------
  console.log("");
  console.log("JSON-RPC 生命周期");
  await wsUpgrade(reader, socket);
  const client = new RpcClient(socket, reader);
  record("WebSocket 升级成功（Sec-WebSocket-Accept 校验通过）", true);

  const beforeInit = await client.call("thread/list");
  const beforeInitKind = beforeInit.error?.data?.kind;
  record(
    "未 initialize 即调用 thread/list 被拒绝",
    beforeInitKind === "unauthorized",
    `kind=${beforeInitKind}`,
  );

  const initialize = await client.call("initialize", {
    protocolVersion: 1,
    deviceId: DEVICE_ID,
    deviceSecret: DEVICE_SECRET,
    client: { name: "verify-harness", platform: "node" },
  });
  const initResult = initialize.result ?? {};
  record(
    "initialize 用设备密钥换取会话",
    initialize.error === undefined &&
      initResult.protocolVersion === 1 &&
      initResult.eventProtocolVersion === 1,
    `protocolVersion=${initResult.protocolVersion} eventProtocolVersion=${initResult.eventProtocolVersion} server=${initResult.server?.name}@${initResult.server?.version}`,
  );
  record(
    "initialize 返回的证书指纹与本地一致",
    initResult.server?.fingerprint === fingerprint.detail,
    initResult.server?.fingerprint,
  );

  const initialized = await client.call("initialized", {});
  record(
    "initialized 之后连接进入 ready",
    initialized.result?.ready === true,
    JSON.stringify(initialized.result),
  );

  const badSecret = await openTls(HOST, PORT);
  const badReader = new Reader(badSecret);
  await wsUpgrade(badReader, badSecret);
  const badClient = new RpcClient(badSecret, badReader);
  const bad = await badClient.call("initialize", {
    deviceId: DEVICE_ID,
    deviceSecret: "wrong-secret",
  });
  record(
    "错误设备密钥被拒绝",
    bad.error?.data?.kind === "unauthorized",
    `kind=${bad.error?.data?.kind}`,
  );
  badClient.close();

  // --- 4. 真实会话数据 -----------------------------------------------------
  console.log("");
  console.log("真实会话数据（来自桌面端 SQLite）");
  const ping = await client.call("ping");
  record(
    "ping 返回服务器时间",
    typeof ping.result?.serverTimeMs === "number",
    `serverTimeMs=${ping.result?.serverTimeMs}`,
  );

  const list = await client.call("thread/list");
  const threads = list.result?.threads ?? [];
  record(
    "thread/list 返回真实会话",
    Array.isArray(threads) && list.error === undefined,
    `${threads.length} 条`,
  );
  for (const thread of threads.slice(0, 5)) {
    console.log(
      `      · ${thread.id}  ${JSON.stringify(thread.title ?? "")}  activeTurn=${thread.activeTurnId ?? "null"}`,
    );
  }

  if (threads.length > 0) {
    const threadId = threads[0].id;
    const subscribe = await client.call("thread/subscribe", { threadId });
    const subscribeCursor = subscribe.result?.deliverySeq;
    record(
      "thread/subscribe 建立事件订阅",
      subscribe.result?.subscribed === true &&
        typeof subscribeCursor === "number",
      `deliverySeq=${subscribeCursor}`,
    );

    // 回归：订阅返回的游标必须能原样喂给 events/resume。
    // 若两者差一，手机端每次重连都会拿到 resume_required 并退化成整段重读。
    const resume = await client.call("events/resume", {
      threadId,
      afterDeliverySeq: subscribeCursor,
    });
    record(
      "订阅游标可直接用于 events/resume（不触发 resume_required）",
      resume.error === undefined &&
        resume.result?.deliverySeq === subscribeCursor,
      resume.error
        ? `kind=${resume.error?.data?.kind} reason=${resume.error?.data?.details?.reason}`
        : `replayed=${resume.result?.replayed} deliverySeq=${resume.result?.deliverySeq}`,
    );

    const read = await client.call("thread/read", { threadId });
    const thread = read.result?.thread;
    const turns = thread?.turns ?? [];
    const itemCount = turns.reduce(
      (total, turn) => total + (turn.items?.length ?? 0),
      0,
    );
    record(
      "thread/read 返回移动安全投影后的会话快照",
      read.error === undefined && Array.isArray(turns),
      `turns=${turns.length} items=${itemCount} title=${JSON.stringify(thread?.title ?? null)}`,
    );
    const leaked = JSON.stringify(thread ?? {}).match(
      /"arguments"|"patch"|"diff"|"fileContent"|"reasoning"/,
    );
    record(
      "投影未泄漏工具参数 / 补丁正文 / 文件内容 / 私有推理",
      leaked === null,
      leaked ? `发现字段 ${leaked[0]}` : "未发现敏感字段",
    );

    const staleCursor = await client.call("events/resume", {
      threadId,
      afterDeliverySeq: subscribeCursor + 100,
    });
    record(
      "越界游标返回 resume_required / cursor_not_buffered",
      staleCursor.error?.data?.kind === "resume_required" &&
        staleCursor.error?.data?.details?.reason === "cursor_not_buffered",
      `kind=${staleCursor.error?.data?.kind} reason=${staleCursor.error?.data?.details?.reason}`,
    );

    const unsubscribe = await client.call("thread/unsubscribe", { threadId });
    record(
      "thread/unsubscribe 取消订阅",
      unsubscribe.result?.subscribed === false,
      "",
    );
  } else {
    record("thread/subscribe / thread/read / events/resume", false, "没有可用会话，跳过");
  }

  // --- 5. 失败路径 ---------------------------------------------------------
  console.log("");
  console.log("失败路径");
  const unknown = await client.call("does/not/exist");
  record(
    "未知方法返回 method_not_found",
    unknown.error?.data?.kind === "method_not_found",
    `kind=${unknown.error?.data?.kind}`,
  );

  const malformed = await client.call("thread/read", {});
  record(
    "缺少必需参数返回 invalid_params",
    malformed.error?.data?.kind === "invalid_params",
    `kind=${malformed.error?.data?.kind}`,
  );

  const fileRead = await client.call("file/read", { path: "AGENTS.md" });
  record(
    "未授予 fileRead 能力时 file/read 被能力门控拦下",
    fileRead.error?.data?.kind === "unsupported_capability",
    `kind=${fileRead.error?.data?.kind} details=${JSON.stringify(fileRead.error?.data?.details)}`,
  );

  const shellRun = await client.call("shell/run", { command: "whoami" });
  record(
    "未授予 shell 能力时 shell/run 被能力门控拦下",
    shellRun.error?.data?.kind === "unsupported_capability",
    `kind=${shellRun.error?.data?.kind}`,
  );

  const badVersion = await openTls(HOST, PORT);
  const badVersionReader = new Reader(badVersion);
  await wsUpgrade(badVersionReader, badVersion);
  const badVersionClient = new RpcClient(badVersion, badVersionReader);
  const future = await badVersionClient.call("initialize", {
    protocolVersion: 99,
    deviceId: DEVICE_ID,
    deviceSecret: DEVICE_SECRET,
  });
  record(
    "更高的协议版本被拒绝",
    future.error?.data?.kind === "unsupported_capability",
    `kind=${future.error?.data?.kind} details=${JSON.stringify(future.error?.data?.details)}`,
  );
  badVersionClient.close();

  // --- 6. 已撤销设备立即失去访问 -------------------------------------------
  console.log("");
  console.log("设备撤销");
  const revokedSocket = await openTls(HOST, PORT);
  const revokedReader = new Reader(revokedSocket);
  await wsUpgrade(revokedReader, revokedSocket);
  const revokedClient = new RpcClient(revokedSocket, revokedReader);
  const revoked = await revokedClient.call("initialize", {
    deviceId: REVOKED_DEVICE_ID,
    deviceSecret: REVOKED_DEVICE_SECRET,
  });
  record(
    "已撤销设备即使密钥正确也无法建立会话",
    revoked.error?.data?.kind === "unauthorized",
    `kind=${revoked.error?.data?.kind} message=${JSON.stringify(revoked.error?.message)}`,
  );
  revokedClient.close();

  client.close();
  socket.destroy();
  printSummary();
  if (results.some((item) => !item.ok)) process.exitCode = 1;
}

// ---------------------------------------------------------------------------
// 真实浏览器里的手机端 PWA
// ---------------------------------------------------------------------------

async function pwa() {
  const { chromium } = await import("playwright");
  const origin = `https://${HOST}:${PORT}`;
  console.log(`目标：${origin}/m/（真实 Chromium，忽略自签证书告警）`);
  console.log("");

  const browser = await chromium.launch();
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 2,
    isMobile: true,
    hasTouch: true,
    userAgent:
      "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140 Mobile Safari/537.36",
  });
  const page = await context.newPage();
  // 只统计「不发失败请求」的那个页面，避免把故意的 400 响应误判成错误。
  const consoleErrors = [];
  const cspViolations = [];
  const watch = (target, sink) => {
    target.on("console", (message) => {
      if (message.type() !== "error") return;
      const text = message.text();
      if (/Content Security Policy|Refused to/i.test(text)) {
        cspViolations.push(text);
      }
      sink.push(text);
    });
    target.on("pageerror", (error) => sink.push(String(error)));
  };
  watch(page, consoleErrors);

  await page.goto(`${origin}/m/`, { waitUntil: "load" });
  record(
    "手机端 PWA 在真实浏览器中加载",
    (await page.title()).length > 0,
    await page.title(),
  );

  const hintWithoutFragment = await page.locator("#pairHint").textContent();
  record(
    "缺少配对链接时给出明确提示并禁用提交",
    (hintWithoutFragment ?? "").includes("没有检测到配对链接") &&
      (await page.locator("#pairButton").isDisabled()),
    hintWithoutFragment?.trim(),
  );

  // 同一文档内只改 hash：手机浏览器不会重新加载页面，页面必须自己再解析一次。
  await page.goto(`${origin}/m/#c=bogus-challenge.s3cr3t`, { waitUntil: "load" });
  const hintAfterHashChange = await page.locator("#pairHint").textContent();
  record(
    "同文档 hash 变化后重新解析配对链接",
    (hintAfterHashChange ?? "").includes("6 位校验码") &&
      (await page.locator("#pairButton").isEnabled()),
    hintAfterHashChange?.trim(),
  );

  const pairPage = await context.newPage();
  const pairConsoleErrors = [];
  watch(pairPage, pairConsoleErrors);
  await pairPage.goto(`${origin}/m/#c=bogus-challenge.s3cr3t`, {
    waitUntil: "load",
  });
  const hintWithFragment = await pairPage.locator("#pairHint").textContent();
  record(
    "带配对链接冷启动时进入校验码输入态",
    (hintWithFragment ?? "").includes("6 位校验码") &&
      (await pairPage.locator("#pairButton").isEnabled()),
    hintWithFragment?.trim(),
  );

  await pairPage.locator("#deviceName").fill("验收浏览器");
  await pairPage.locator("#pairCode").fill("000000");
  await pairPage.locator("#pairButton").click();
  const banner = pairPage.locator("#banner");
  await banner.waitFor({ state: "visible", timeout: 8000 }).catch(() => {});
  const bannerText = (await banner.textContent()) ?? "";
  record(
    "页面自身发起的 POST /pair 能拿到服务端错误并展示",
    bannerText.trim().length > 0,
    bannerText.trim(),
  );

  // CSP 是 `default-src 'none'; connect-src 'self'`，必须确认浏览器端
  // 仍然允许同源 wss 升级，否则手机端根本连不上。
  const socketProbe = await page.evaluate(
    ([deviceId, deviceSecret]) =>
      new Promise((resolve) => {
        let settled = false;
        const finish = (value) => {
          if (settled) return;
          settled = true;
          resolve(value);
        };
        let socket;
        try {
          socket = new WebSocket(`wss://${location.host}/ws`);
        } catch (error) {
          finish({ ok: false, detail: `构造失败：${error.message}` });
          return;
        }
        const timer = setTimeout(() => finish({ ok: false, detail: "超时" }), 10000);
        socket.onerror = () => {
          clearTimeout(timer);
          finish({ ok: false, detail: "WebSocket error 事件（可能被 CSP 拦截）" });
        };
        socket.onopen = () => {
          socket.send(
            JSON.stringify({
              jsonrpc: "2.0",
              id: "probe-1",
              method: "initialize",
              params: { protocolVersion: 1, deviceId, deviceSecret },
            }),
          );
        };
        socket.onmessage = (event) => {
          const message = JSON.parse(event.data);
          if (message.id !== "probe-1") return;
          clearTimeout(timer);
          finish({
            ok: message.result?.protocolVersion === 1,
            detail: JSON.stringify(message.result?.server ?? message.error),
          });
          socket.close();
        };
      }),
    [DEVICE_ID, DEVICE_SECRET],
  );
  record(
    "浏览器端同源 wss 升级与 initialize 未被 CSP 拦截",
    socketProbe.ok,
    socketProbe.detail,
  );

  record(
    "控制台无报错（未发失败请求的页面）",
    consoleErrors.length === 0,
    consoleErrors.length === 0 ? "" : consoleErrors.slice(0, 3).join(" | "),
  );
  record(
    "两个页面都没有 CSP 违规",
    cspViolations.length === 0,
    cspViolations.length === 0 ? "" : cspViolations.slice(0, 3).join(" | "),
  );
  const unexpectedOnPairPage = pairConsoleErrors.filter(
    (text) => !/Failed to load resource/i.test(text),
  );
  record(
    "配对页除预期的 400 响应外无其他报错",
    unexpectedOnPairPage.length === 0,
    unexpectedOnPairPage.slice(0, 3).join(" | "),
  );

  const shot = path.join(
    process.cwd(),
    "docs",
    "手机端页面-真实浏览器.png",
  );
  await page.goto(`${origin}/m/`, { waitUntil: "load" });
  await page.screenshot({ path: shot, fullPage: true });
  console.log(`  截图已保存：${shot}`);

  await browser.close();
  printSummary();
  if (results.some((item) => !item.ok)) process.exitCode = 1;
}

function printSummary() {
  const passed = results.filter((item) => item.ok).length;
  console.log("");
  console.log(`汇总：${passed} / ${results.length} 通过`);
  for (const item of results.filter((entry) => !entry.ok)) {
    console.log(`  未通过：${item.name} — ${item.detail}`);
  }
}

// ---------------------------------------------------------------------------

const command = process.argv[2];
switch (command) {
  case "seed":
    seed();
    break;
  case "phone":
    await phone();
    break;
  case "pwa":
    await pwa();
    break;
  case "restore":
    restore();
    break;
  case "paths":
    console.log(
      JSON.stringify(
        { DATA_ROOT, STATE_PATH, CERT_PATH, HOST, PORT },
        null,
        2,
      ),
    );
    break;
  default:
    console.log(
      "用法：node scripts/verify-mobile-native.mjs <seed|phone|pwa|restore|paths>",
    );
    process.exitCode = 1;
}
