#!/usr/bin/env node
'use strict';
/**
 * CCMS router — Anthropic Messages 协议路由器。
 * 档位分发 + 密钥池轮换 + 失败重试 + 用量统计。
 */

const http = require('http');
const https = require('https');
const fs = require('fs');
const path = require('path');
const os = require('os');

const HOME = process.env.HOME || os.homedir();
const CFG = path.join(HOME, '.config', 'ccms');
const ROUTES_FILE = path.join(CFG, 'routes.json');
const KEYS_FILE = path.join(CFG, 'keys.env');
const PROVIDERS_FILE = path.join(CFG, 'providers.json');
const LOG_FILE = path.join(CFG, 'router.log');
const STATS_FILE = path.join(CFG, 'stats.json');
const PID_FILE = path.join(CFG, 'router.pid');

const RETRY_DEFAULTS = { attempts: 3, backoff_ms: 400, timeout_ms: 120000, connect_timeout_ms: 15000, retry_on: [401, 403, 408, 429, 500, 502, 503, 504] };

const DEFAULT_ROUTES = {
  listen: '127.0.0.1:8788',
  default: 'sonnet',
  routes: {
    opus: { provider: 'kimi', model: 'kimi-k3.1-pro' },
    sonnet: { provider: 'qwen', model: 'qwen3-coder-plus' },
    haiku: { provider: 'deepseek', model: 'deepseek-flash' },
    fable: { provider: 'kimi', model: 'kimi-k3.1-pro' },
    subagent: { provider: 'deepseek', model: 'deepseek-flash' },
  },
  retry: RETRY_DEFAULTS,
};

function readJSON(file, fallback) {
  try { return JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return fallback; }
}

function readKeys() {
  const out = {};
  try {
    for (const line of fs.readFileSync(KEYS_FILE, 'utf8').split(/\r?\n/)) {
      const m = line.match(/^\s*([A-Za-z0-9_]+)\s*=\s*(.*?)\s*$/);
      if (m) out[m[1]] = m[2].replace(/^["']|["']$/g, '');
    }
  } catch {}
  return out;
}

function log(line) {
  const ts = new Date().toISOString();
  try { fs.appendFileSync(LOG_FILE, `[${ts}] ${line}\n`); } catch {}
}

function loadConfig() {
  const cfg = readJSON(ROUTES_FILE, DEFAULT_ROUTES);
  if (!cfg.routes || typeof cfg.routes !== 'object') cfg.routes = DEFAULT_ROUTES.routes;
  if (!cfg.default) cfg.default = 'sonnet';
  if (!cfg.listen) cfg.listen = '127.0.0.1:8788';
  cfg.retry = Object.assign({}, RETRY_DEFAULTS, cfg.retry || {});
  if (!Array.isArray(cfg.retry.retry_on)) cfg.retry.retry_on = RETRY_DEFAULTS.retry_on;
  return cfg;
}

const providers = readJSON(PROVIDERS_FILE, {});
const config = loadConfig();
const [HOST, PORT_STR] = [config.listen.split(':')[0], config.listen.split(':').slice(1).join(':')];
const PORT = parseInt(PORT_STR, 10);
const retrySet = new Set(config.retry.retry_on.map(Number));

function poolVar(p) {
  return p.key_env_pool || String(p.key_env || '').replace(/_KEY$/, '_KEYS');
}

function keysFor(providerName) {
  const p = providers[providerName];
  if (!p) return [];
  const env = readKeys();
  const raw = env[poolVar(p)] || '';
  let list = raw.split(/[,\s]+/).map((s) => s.trim()).filter(Boolean);
  if (!list.length && env[p.key_env]) list = [String(env[p.key_env]).trim()];
  return list.filter((k) => k && !k.includes('REPLACE_ME'));
}

const keyCursor = new Map();
function takeStart(providerName, n) {
  const cur = keyCursor.get(providerName) || 0;
  keyCursor.set(providerName, (cur + 1) % n);
  return cur % n;
}

function tierFor(model) {
  const m = String(model || '').toLowerCase();
  if (m && config.routes[m]) return m;
  if (/opus/.test(m)) return 'opus';
  if (/haiku/.test(m)) return 'haiku';
  if (/fable/.test(m)) return 'fable';
  if (/sonnet/.test(m)) return 'sonnet';
  if (/subagent/.test(m)) return 'subagent';
  return config.default;
}

function routeFor(tier) {
  return config.routes[tier] || config.routes[config.default] || null;
}

function upstreamUrl(base, reqUrl) {
  const b = String(base || '').replace(/\/+$/, '');
  return new URL(b + reqUrl);
}

function pickHeaders(headers, token, bodyBuf) {
  const out = {};
  for (const [k, v] of Object.entries(headers)) {
    const lk = k.toLowerCase();
    if (['host', 'authorization', 'x-api-key', 'content-length', 'connection', 'accept-encoding'].includes(lk)) continue;
    out[k] = v;
  }
  out['authorization'] = `Bearer ${token}`;
  out['content-length'] = Buffer.byteLength(bodyBuf);
  out['accept-encoding'] = 'identity';
  return out;
}

function sendJSON(res, code, obj) {
  if (res.headersSent) { try { res.destroy(); } catch {} return; }
  const buf = Buffer.from(JSON.stringify(obj));
  res.writeHead(code, { 'content-type': 'application/json', 'content-length': buf.length });
  res.end(buf);
}

// ---------- usage stats ----------
function blankBucket() {
  return { requests: 0, input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, retries: 0 };
}

function addTo(bucket, u) {
  bucket.requests += 1;
  bucket.input_tokens += Number(u.input_tokens) || 0;
  bucket.output_tokens += Number(u.output_tokens) || 0;
  bucket.cache_read_input_tokens += Number(u.cache_read_input_tokens) || 0;
  bucket.cache_creation_input_tokens += Number(u.cache_creation_input_tokens) || 0;
  bucket.retries += Number(u.retries) || 0;
}

function localDay(d = new Date()) {
  const p = (x) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function recordUsage(tier, provider, usage) {
  try {
    const day = localDay();
    const data = readJSON(STATS_FILE, { version: 1, daily: {} });
    if (!data.daily) data.daily = {};
    const d = data.daily[day] || (data.daily[day] = { total: blankBucket(), providers: {}, tiers: {} });
    if (!d.total) d.total = blankBucket();
    if (!d.providers) d.providers = {};
    if (!d.tiers) d.tiers = {};
    addTo(d.total, usage);
    if (!d.providers[provider]) d.providers[provider] = blankBucket();
    addTo(d.providers[provider], usage);
    if (!d.tiers[tier]) d.tiers[tier] = blankBucket();
    addTo(d.tiers[tier], usage);

    const days = Object.keys(data.daily).sort();
    while (days.length > 180) delete data.daily[days.shift()];

    const tmp = `${STATS_FILE}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(data));
    fs.renameSync(tmp, STATS_FILE);
  } catch (e) {
    log(`stats error: ${e.message}`);
  }
}

function extractUsage(obj) {
  const u = (obj && obj.usage) || null;
  if (!u) return null;
  return {
    input_tokens: Number(u.input_tokens) || 0,
    output_tokens: Number(u.output_tokens) || 0,
    cache_read_input_tokens: Number(u.cache_read_input_tokens) || 0,
    cache_creation_input_tokens: Number(u.cache_creation_input_tokens) || 0,
  };
}

const startTime = Date.now();

const server = http.createServer((req, res) => {
  res.setTimeout(0);
  req.setTimeout(0);

  if (req.url === '/ccms/health' || req.url === '/ccms/health/') {
    const keys = {};
    for (const name of Object.keys(providers)) keys[name] = keysFor(name).length;
    return sendJSON(res, 200, {
      ok: true,
      uptime_s: Math.round((Date.now() - startTime) / 1000),
      listen: config.listen,
      default: config.default,
      routes: config.routes,
      retry: config.retry,
      keys,
    });
  }

  if (req.method === 'GET' && req.url.startsWith('/v1/models')) {
    const data = Object.entries(config.routes).map(([tier, r]) => ({
      id: `ccms-${tier}`,
      type: 'model',
      display_name: `ccms ${tier} → ${r.provider}/${r.model}`,
      created_at: new Date().toISOString(),
    }));
    return sendJSON(res, 200, { data, has_more: false, first_id: data[0]?.id, last_id: data[data.length - 1]?.id });
  }

  const chunks = [];
  req.on('data', (c) => chunks.push(c));
  req.on('error', (e) => { log(`req error ${e.message}`); try { res.destroy(); } catch {} });
  req.on('end', () => {
    let bodyBuf = Buffer.concat(chunks);
    let body = null;
    if (bodyBuf.length) {
      try { body = JSON.parse(bodyBuf.toString('utf8')); } catch { body = null; }
    }

    const tier = tierFor(body && body.model);
    const route = routeFor(tier);
    const started = Date.now();

    if (!route) {
      log(`${tier} 无路由配置 → 503`);
      return sendJSON(res, 503, { type: 'error', error: { type: 'api_error', message: `ccms-router: 档位 ${tier} 没有配置路由，编辑 ${ROUTES_FILE}` } });
    }

    const p = providers[route.provider];
    if (!p) {
      log(`${tier} provider ${route.provider} 不存在 → 503`);
      return sendJSON(res, 503, { type: 'error', error: { type: 'api_error', message: `ccms-router: provider ${route.provider} 未在 providers.json 中定义` } });
    }

    const keys = keysFor(route.provider);
    if (!keys.length) {
      log(`${tier} ${route.provider} 密钥池为空 → 401`);
      return sendJSON(res, 401, { type: 'error', error: { type: 'authentication_error', message: `ccms-router: ${route.provider} 没有可用 key，运行 ccms --key ${route.provider} sk-xxx` } });
    }

    if (body && body.model) body.model = route.model;
    if (body) bodyBuf = Buffer.from(JSON.stringify(body));

    let target;
    try { target = upstreamUrl(p.base_url, req.url); }
    catch (e) {
      log(`${tier} URL 非法: ${e.message}`);
      return sendJSON(res, 500, { type: 'error', error: { type: 'api_error', message: `ccms-router: base_url 非法 ${p.base_url}` } });
    }

    const isCountTokens = req.url.includes('/count_tokens');
    const lib = target.protocol === 'http:' ? http : https;
    const maxAttempts = Math.max(1, Number(config.retry.attempts) || 1);
    const backoffMs = Math.max(0, Number(config.retry.backoff_ms) || 0);
    const startIdx = takeStart(route.provider, keys.length);

    let attempt = 0;
    let clientClosed = false;
    let current = null;
    let recorded = false;

    res.on('close', () => {
      clientClosed = true;
      if (current) { try { current.destroy(); } catch {} }
    });

    function finish(usage) {
      if (recorded) return;
      recorded = true;
      const u = Object.assign({ retries: Math.max(0, attempt - 1) }, usage || {});
      recordUsage(tier, route.provider, u);
    }

    function retryLater(status, keyIdx, e) {
      if (attempt >= maxAttempts || clientClosed || res.writableEnded) return false;
      const delay = status === 401 ? 50 : Math.min(backoffMs * attempt, 5000);
      const why = e ? `ERR ${e.message}` : `HTTP ${status}`;
      log(`${tier} ${route.provider}/${route.model} ${why} key#${keyIdx + 1}/${keys.length} → 重试 ${attempt + 1}/${maxAttempts} in ${delay}ms`);
      setTimeout(runAttempt, delay);
      return true;
    }

    function runAttempt() {
      if (clientClosed || res.writableEnded) return;
      attempt += 1;
      const keyIdx = (startIdx + attempt - 1) % keys.length;
      const token = keys[keyIdx];

      const upReq = lib.request(
        {
          protocol: target.protocol,
          hostname: target.hostname,
          port: target.port || (target.protocol === 'http:' ? 80 : 443),
          path: target.pathname + target.search,
          method: req.method,
          headers: pickHeaders(req.headers, token, bodyBuf),
        },
        (upRes) => {
          const status = upRes.statusCode || 502;

          if (isCountTokens && (status === 404 || status === 405)) {
            upRes.resume();
            const input = Math.max(1, Math.ceil(bodyBuf.toString('utf8').length / 4));
            log(`${tier} ${route.provider}/${route.model} count_tokens ${status} → 本地估算 ${input}`);
            return sendJSON(res, 200, { input_tokens: input });
          }

          if (retrySet.has(status) && retryLater(status, keyIdx, null)) {
            upRes.resume();
            return;
          }

          const outHeaders = { ...upRes.headers };
          for (const h of ['content-length', 'transfer-encoding', 'connection', 'keep-alive']) delete outHeaders[h];
          res.writeHead(status, outHeaders);

          const ctype = String(upRes.headers['content-type'] || '');
          const isSSE = ctype.includes('text/event-stream');
          let usage = null;

          if (isSSE) {
            let buf = '';
            upRes.on('data', (chunk) => {
              res.write(chunk);
              buf += chunk.toString('utf8');
              let idx;
              while ((idx = buf.indexOf('\n\n')) >= 0) {
                const block = buf.slice(0, idx);
                buf = buf.slice(idx + 2);
                for (const line of block.split('\n')) {
                  if (!line.startsWith('data:')) continue;
                  const payload = line.slice(5).trim();
                  if (!payload || payload === '[DONE]') continue;
                  try {
                    const ev = JSON.parse(payload);
                    const u = (ev.type === 'message_start' && ev.message && ev.message.usage)
                      || (ev.type === 'message_delta' && ev.usage)
                      || null;
                    if (u) {
                      usage = usage || { input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 };
                      usage.input_tokens = Math.max(usage.input_tokens, Number(u.input_tokens) || 0);
                      usage.output_tokens = Math.max(usage.output_tokens, Number(u.output_tokens) || 0);
                      usage.cache_read_input_tokens = Math.max(usage.cache_read_input_tokens, Number(u.cache_read_input_tokens) || 0);
                      usage.cache_creation_input_tokens = Math.max(usage.cache_creation_input_tokens, Number(u.cache_creation_input_tokens) || 0);
                    }
                  } catch {}
                }
              }
            });
            upRes.on('end', () => {
              res.end();
              finish(usage);
              log(`${tier} ${route.provider}/${route.model} ${status} ${Date.now() - started}ms key#${keyIdx + 1}/${keys.length}`);
            });
          } else {
            const bufs = [];
            upRes.on('data', (chunk) => { bufs.push(chunk); res.write(chunk); });
            upRes.on('end', () => {
              res.end();
              let obj = null;
              try { obj = JSON.parse(Buffer.concat(bufs).toString('utf8')); } catch {}
              const u = extractUsage(obj);
              finish(u);
              log(`${tier} ${route.provider}/${route.model} ${status} ${Date.now() - started}ms key#${keyIdx + 1}/${keys.length}`);
            });
          }

          upRes.on('error', (e) => {
            log(`${tier} ${route.provider}/${route.model} stream ERR ${e.message}`);
            try { res.destroy(); } catch {}
          });
        }
      );

      current = upReq;
      const headTimeout = Math.max(5000, Number(config.retry.timeout_ms) || 120000);
      const connTimeout = Math.max(1000, Number(config.retry.connect_timeout_ms) || 15000);
      let connTimer = null;
      const timer = setTimeout(() => {
        upReq.destroy(new Error(`等待响应超时 ${Math.round(headTimeout / 1000)}s`));
      }, headTimeout);
      const clearTimer = () => { clearTimeout(timer); if (connTimer) clearTimeout(connTimer); };
      upReq.on('socket', (socket) => {
        if (!socket.connecting) return;
        connTimer = setTimeout(() => {
          upReq.destroy(new Error(`连接超时 ${Math.round(connTimeout / 1000)}s`));
        }, connTimeout);
        socket.once('connect', () => clearTimeout(connTimer));
        socket.once('error', () => clearTimeout(connTimer));
      });
      upReq.on('response', clearTimer);
      upReq.on('close', clearTimer);
      upReq.on('error', (e) => {
        clearTimer();
        current = null;
        if (retryLater(0, keyIdx, e)) return;
        log(`${tier} ${route.provider}/${route.model} ERR ${e.message}（已重试 ${attempt}/${maxAttempts}）`);
        finish(null);
        if (!res.headersSent) sendJSON(res, 502, { type: 'error', error: { type: 'api_error', message: `ccms-router: 连不上 ${route.provider} (${e.message})` } });
        else res.destroy();
      });

      if (bodyBuf.length) upReq.write(bodyBuf);
      upReq.end();
    }

    runAttempt();
  });
});

server.on('error', (e) => {
  if (e.code === 'EADDRINUSE') {
    console.log(`ccms router: ${config.listen} 已被占用（可能已经在运行）`);
    process.exit(0);
  }
  console.error(`ccms router: ${e.message}`);
  process.exit(1);
});

server.listen(PORT, HOST, () => {
  try { fs.writeFileSync(PID_FILE, String(process.pid)); } catch {}
  const r = Object.entries(config.routes).map(([t, v]) => `${t}=${v.provider}/${v.model}`).join('  ');
  console.log(`ccms router listening on http://${config.listen}  retry=${config.retry.attempts}x  ${r}`);
  log(`router start pid=${process.pid} listen=${config.listen} retry=${config.retry.attempts} ${r}`);
});

function shutdown(sig) {
  console.log(`ccms router: ${sig}, bye`);
  try { fs.unlinkSync(PID_FILE); } catch {}
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 1500).unref();
}
process.on('SIGINT', () => shutdown('SIGINT'));
process.on('SIGTERM', () => shutdown('SIGTERM'));
