#!/usr/bin/env node
'use strict';
/**
 * CCMS keystore — 加密密钥池。
 *
 * 把原本明文 keys.env 的密钥池加密到 keys.enc（AES-256-GCM），主密钥与密文分离：
 *   - keyfile 模式（默认）：32 字节随机主密钥存于 keystore.key（权限 600）
 *   - passphrase 模式：由 CCMS_KEYSTORE_PASSPHRASE 经 scrypt 派生，主密钥不落盘
 *
 * 两种用法：
 *   - 被 router.js require：exists() / open() / status()
 *   - 被 bin/ccms 当 CLI 调用：open / get / set / seal / status / rekey / path
 * 只用 Node 内置模块，无第三方依赖。
 *
 * 环境变量（CLI 与 router 必须一致）：
 *   CCMS_HOME                   配置目录，默认 $HOME/.config/ccms
 *   CCMS_KEYSTORE_ENC           覆盖 keys.enc 路径
 *   CCMS_KEYSTORE_KEYFILE       覆盖 keystore.key 路径
 *   CCMS_KEYSTORE_LEGACY        覆盖 keys.env 路径
 *   CCMS_KEYSTORE_PASSPHRASE    口令模式的主口令
 */

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const os = require('os');

const HOME = process.env.HOME || os.homedir();
const CFG = process.env.CCMS_HOME || path.join(HOME, '.config', 'ccms');
const ENC_FILE = process.env.CCMS_KEYSTORE_ENC || path.join(CFG, 'keys.enc');
const KEY_FILE = process.env.CCMS_KEYSTORE_KEYFILE || path.join(CFG, 'keystore.key');
const LEGACY_FILE = process.env.CCMS_KEYSTORE_LEGACY || path.join(CFG, 'keys.env');

const ALGO = 'aes-256-gcm';
const VERSION = 1;
const KEY_BYTES = 32;

// ---------- helpers ----------

function exists() { return fs.existsSync(ENC_FILE); }

function passphrase() {
  const p = process.env.CCMS_KEYSTORE_PASSPHRASE;
  return typeof p === 'string' && p.length ? p : null;
}

function readMeta() {
  let raw;
  try { raw = JSON.parse(fs.readFileSync(ENC_FILE, 'utf8')); }
  catch (e) { throw new Error(`密钥库读取失败: ${e.message}`); }
  if (!raw || raw.version !== VERSION) {
    throw new Error(`不支持的密钥库版本 ${raw && raw.version}`);
  }
  const mode = raw.mode === 'passphrase' ? 'passphrase' : 'keyfile';
  return { raw, mode, salt: raw.salt ? Buffer.from(raw.salt, 'base64') : null };
}

function readKeyfile() {
  let txt;
  try { txt = fs.readFileSync(KEY_FILE, 'utf8').trim(); }
  catch (e) {
    if (e.code === 'ENOENT') {
      throw new Error(`主密钥文件缺失: ${KEY_FILE}（密钥库必须与它的主密钥一起备份/迁移）`);
    }
    throw e;
  }
  const b = Buffer.from(txt, 'base64');
  if (b.length !== KEY_BYTES) {
    throw new Error(`主密钥文件损坏: ${KEY_FILE}（应为 ${KEY_BYTES} 字节 base64）`);
  }
  return b;
}

function ensureKeyfile() {
  if (fs.existsSync(KEY_FILE)) return readKeyfile();
  const b = crypto.randomBytes(KEY_BYTES);
  writeAtomic(KEY_FILE, b.toString('base64') + '\n', 0o600);
  return b;
}

function keyFor(mode, salt, create) {
  if (mode === 'passphrase') {
    const p = passphrase();
    if (!p) throw new Error('该密钥库使用口令模式，但未设置 CCMS_KEYSTORE_PASSPHRASE');
    if (!salt) throw new Error('密钥库缺少 scrypt salt');
    return crypto.scryptSync(p, salt, KEY_BYTES);
  }
  return create ? ensureKeyfile() : readKeyfile();
}

// 把元数据绑进 GCM 的 AAD：篡改 version/mode 会让校验失败而不是被静默接受。
function aadOf(mode) { return Buffer.from(`ccms-keystore:${VERSION}:${mode}`, 'utf8'); }

function writeAtomic(file, text, mode) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = `${file}.tmp`;
  fs.writeFileSync(tmp, text, { mode });
  try { fs.chmodSync(tmp, mode); } catch {}
  fs.renameSync(tmp, file);
  try { fs.chmodSync(file, mode); } catch {}
}

// ---------- crypto ----------

function encrypt(obj, key, mode, salt) {
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv(ALGO, key, iv);
  cipher.setAAD(aadOf(mode));
  const plain = Buffer.from(JSON.stringify(obj), 'utf8');
  const data = Buffer.concat([cipher.update(plain), cipher.final()]);
  const out = {
    version: VERSION,
    cipher: ALGO,
    mode,
    iv: iv.toString('base64'),
    tag: cipher.getAuthTag().toString('base64'),
    data: data.toString('base64'),
    updated: new Date().toISOString(),
  };
  if (mode === 'passphrase') {
    out.kdf = 'scrypt';
    out.salt = salt.toString('base64');
  }
  writeAtomic(ENC_FILE, JSON.stringify(out, null, 2) + '\n', 0o600);
}

function decrypt() {
  const { raw, mode, salt } = readMeta();
  const key = keyFor(mode, salt, false);
  try {
    const decipher = crypto.createDecipheriv(ALGO, key, Buffer.from(raw.iv, 'base64'));
    decipher.setAAD(aadOf(mode));
    decipher.setAuthTag(Buffer.from(raw.tag, 'base64'));
    const plain = Buffer.concat([
      decipher.update(Buffer.from(raw.data, 'base64')),
      decipher.final(),
    ]);
    const obj = JSON.parse(plain.toString('utf8'));
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) {
      throw new Error('内容不是对象');
    }
    return obj;
  } catch (e) {
    throw new Error('解密失败：主密钥不匹配、口令错误或密钥库已损坏');
  }
}

// ---------- plaintext side ----------

function parseEnvText(text) {
  const out = {};
  for (const line of String(text).split(/\r?\n/)) {
    const m = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$/);
    if (!m) continue;
    out[m[1]] = m[2].replace(/^["']|["']$/g, '');
  }
  return out;
}

function readLegacy() {
  try { return parseEnvText(fs.readFileSync(LEGACY_FILE, 'utf8')); }
  catch { return {}; }
}

// 未加密时读明文，已加密时解密：写入路径靠它实现无缝迁移。
function loadPlain() { return exists() ? decrypt() : readLegacy(); }

function shellQuote(v) {
  return "'" + String(v).replace(/'/g, "'\\''") + "'";
}

function toEnvText(obj) {
  const lines = Object.keys(obj).sort().map((k) => `${k}=${shellQuote(obj[k])}`);
  return lines.length ? lines.join('\n') + '\n' : '';
}

// 迁移后抹掉明文，但保留文件，避免依赖「文件存在」的旧逻辑报错。
function neutralizeLegacy() {
  let txt;
  try { txt = fs.readFileSync(LEGACY_FILE, 'utf8'); }
  catch { return; }
  if (!/^\s*[A-Za-z_][A-Za-z0-9_]*\s*=\s*\S/m.test(txt)) return;
  writeAtomic(LEGACY_FILE,
    '# 密钥已迁移到 keys.enc（AES-256-GCM），此文件不再保存明文。\n' +
    '# 重新导入：手动填入 VAR=值 后执行 ccms --vault --seal\n', 0o600);
}

function writePlain(obj) {
  let mode, salt, key;
  if (exists()) {
    // 已有密钥库：沿用原模式。否则一次偶然的环境变量就会把后台 router 锁在外面。
    const meta = readMeta();
    mode = meta.mode;
    salt = meta.salt;
    key = keyFor(mode, salt, false);
  } else {
    mode = passphrase() ? 'passphrase' : 'keyfile';
    salt = mode === 'passphrase' ? crypto.randomBytes(16) : null;
    key = mode === 'passphrase' ? keyFor(mode, salt, false) : ensureKeyfile();
  }
  encrypt(obj, key, mode, salt);
  neutralizeLegacy();
}

// ---------- public operations ----------

function get(name) {
  const obj = loadPlain();
  return Object.prototype.hasOwnProperty.call(obj, name) ? obj[name] : '';
}

function set(name, value) {
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
    throw new Error(`非法变量名: ${name}`);
  }
  const obj = loadPlain();
  obj[name] = String(value);
  writePlain(obj);
}

function seal() {
  const legacy = readLegacy();
  const current = exists() ? decrypt() : {};
  // 已加密的值优先，避免旧明文文件覆盖较新的密钥池。
  const merged = Object.assign({}, legacy, current);
  writePlain(merged);
  return { imported: Object.keys(legacy).length, entries: Object.keys(merged).length };
}

function rekey() {
  const obj = exists() ? decrypt() : readLegacy();
  const mode = passphrase() ? 'passphrase' : 'keyfile';
  const salt = mode === 'passphrase' ? crypto.randomBytes(16) : null;
  const key = mode === 'passphrase' ? keyFor(mode, salt, false) : crypto.randomBytes(KEY_BYTES);

  // 轮换会同时替换密文与主密钥；留一份旧字节，任何一步失败都整体回滚。
  let oldEnc = null, oldKey = null;
  try { oldEnc = fs.readFileSync(ENC_FILE); } catch {}
  try { oldKey = fs.readFileSync(KEY_FILE); } catch {}

  try {
    encrypt(obj, key, mode, salt);
    if (mode === 'keyfile') {
      writeAtomic(KEY_FILE, key.toString('base64') + '\n', 0o600);
    }
  } catch (e) {
    try {
      if (oldEnc) fs.writeFileSync(ENC_FILE, oldEnc, { mode: 0o600 });
      else fs.unlinkSync(ENC_FILE);
    } catch {}
    try { if (oldKey) fs.writeFileSync(KEY_FILE, oldKey, { mode: 0o600 }); } catch {}
    throw new Error(`主密钥轮换失败，已回滚: ${e.message}`);
  }
  neutralizeLegacy();
  return { mode, entries: Object.keys(obj).length };
}

function status() {
  if (!exists()) {
    return '存储方式    明文 keys.env（未启用加密）\n' +
           '建议        ccms --vault --seal';
  }
  const { raw, mode } = readMeta();
  let entries = 0, decryptable = false;
  try { entries = Object.keys(decrypt()).length; decryptable = true; } catch {}
  const lines = [];
  lines.push(`存储方式    ${path.basename(ENC_FILE)}  ${raw.cipher || ALGO}`);
  lines.push(`主密钥模式  ${mode}${mode === 'passphrase' ? '（CCMS_KEYSTORE_PASSPHRASE / scrypt）' : ''}`);
  if (mode === 'keyfile') {
    let perm = '缺失';
    try { perm = (fs.statSync(KEY_FILE).mode & 0o777).toString(8); } catch {}
    lines.push(`主密钥文件  ${KEY_FILE}  ${perm}`);
  } else {
    lines.push('主密钥文件  （口令派生，不落盘）');
  }
  lines.push(`密钥条目    ${decryptable ? entries : '解密失败'}`);
  lines.push(`可解密      ${decryptable ? '是' : '否（主密钥/口令不匹配）'}`);
  lines.push(`更新于      ${raw.updated || '未知'}`);
  return lines.join('\n');
}

module.exports = {
  exists,
  open: decrypt,
  status,
  set,
  seal,
  rekey,
  paths: { ENC_FILE, KEY_FILE, LEGACY_FILE },
};

// ---------- CLI ----------

function readStdin() {
  try { return fs.readFileSync(0, 'utf8').replace(/\r?\n$/, ''); }
  catch { return ''; }
}

function main() {
  const [cmd, arg] = process.argv.slice(2);
  switch (cmd) {
    case 'open':
      process.stdout.write(toEnvText(loadPlain()));
      break;
    case 'get':
      process.stdout.write(get(arg || '') + '\n');
      break;
    case 'set': {
      if (!arg) throw new Error('用法: keystore.js set <VAR>（值从 stdin 读）');
      set(arg, readStdin());   // 值走 stdin，不出现在 argv / ps 里
      break;
    }
    case 'seal': {
      const r = seal();
      process.stderr.write(`已加密 ${r.entries} 个条目（导入明文 ${r.imported} 个）\n`);
      break;
    }
    case 'rekey': {
      const r = rekey();
      process.stderr.write(`主密钥已轮换：${r.mode}，${r.entries} 个条目\n`);
      break;
    }
    case 'status':
      process.stdout.write(status() + '\n');
      break;
    case 'path':
      process.stdout.write(ENC_FILE + '\n');
      break;
    default:
      throw new Error('用法: keystore.js <open|get|set|seal|rekey|status|path>');
  }
}

if (require.main === module) {
  try { main(); }
  catch (e) { console.error(`✗ ${e.message}`); process.exit(1); }
}
