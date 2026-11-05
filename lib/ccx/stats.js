#!/usr/bin/env node
'use strict';
/**
 * CCMS stats — 读取 ~/.config/ccx/stats.json 并汇总输出。
 * 用法: node stats.js [天数|reset] [--json]
 */

const fs = require('fs');
const os = require('os');
const path = require('path');

const HOME = process.env.HOME || os.homedir();
const CFG = path.join(HOME, '.config', 'ccx');
const STATS_FILE = path.join(CFG, 'stats.json');

const args = process.argv.slice(2);
const jsonOut = args.includes('--json');
const reset = args.includes('reset');
const numArg = args.find((a) => /^\d+$/.test(a));
const days = Math.max(1, parseInt(numArg || '7', 10));

function readJSON(file, fallback) {
  try { return JSON.parse(fs.readFileSync(file, 'utf8')); } catch { return fallback; }
}

if (reset) {
  try { fs.unlinkSync(STATS_FILE); } catch {}
  console.log('已清空用量记录');
  process.exit(0);
}

const data = readJSON(STATS_FILE, { daily: {} });
const daily = data.daily || {};

function localDay(d = new Date()) {
  const p = (x) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
const today = localDay();

function empty() {
  return { requests: 0, input_tokens: 0, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0, retries: 0 };
}
function add(a, b) {
  for (const k of Object.keys(empty())) a[k] += Number(b[k]) || 0;
  return a;
}
function sumBuckets(objs) {
  const out = empty();
  for (const o of objs) if (o) add(out, o);
  return out;
}

function cutoffDate(n) {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() - (n - 1));
  return localDay(d);
}
const cutoff = cutoffDate(days);
const dayKeys = Object.keys(daily).filter((d) => d >= cutoff).sort();

const rangeTotal = sumBuckets(dayKeys.map((d) => daily[d].total));
const todayTotal = sumBuckets([daily[today] && daily[today].total]);

const byProvider = {};
const byTier = {};
for (const d of dayKeys) {
  const day = daily[d] || {};
  for (const [k, v] of Object.entries(day.providers || {})) {
    if (!byProvider[k]) byProvider[k] = empty();
    add(byProvider[k], v);
  }
  for (const [k, v] of Object.entries(day.tiers || {})) {
    if (!byTier[k]) byTier[k] = empty();
    add(byTier[k], v);
  }
}

if (jsonOut) {
  console.log(JSON.stringify({ today, days, from: cutoff, rangeTotal, todayTotal, byProvider, byTier }, null, 2));
  process.exit(0);
}

const n = (x) => Number(x || 0).toLocaleString('en-US');
const pad = (s, w) => String(s).padEnd(w);
const padL = (s, w) => String(s).padStart(w);

if (!dayKeys.length) {
  console.log('暂无用量记录（启用路由并产生请求后自动统计）');
  process.exit(0);
}

function block(title, b) {
  console.log(title);
  console.log(`  ${pad('requests', 10)}${padL(n(b.requests), 14)}`);
  console.log(`  ${pad('input', 10)}${padL(n(b.input_tokens), 14)}`);
  console.log(`  ${pad('output', 10)}${padL(n(b.output_tokens), 14)}`);
  if (b.cache_read_input_tokens || b.cache_creation_input_tokens) {
    console.log(`  ${pad('cache r/w', 10)}${padL(`${n(b.cache_read_input_tokens)} / ${n(b.cache_creation_input_tokens)}`, 14)}`);
  }
  if (b.retries) console.log(`  ${pad('retries', 10)}${padL(n(b.retries), 14)}`);
}

block(`Today  ${today}`, todayTotal);
console.log('');
block(`Last ${days} days  (${cutoff} ~ ${today})`, rangeTotal);

if (Object.keys(byProvider).length) {
  console.log('\nBy provider');
  console.log(`  ${pad('PROVIDER', 12)}${padL('REQ', 7)}${padL('INPUT', 14)}${padL('OUTPUT', 12)}`);
  for (const [k, v] of Object.entries(byProvider).sort((a, b) => b[1].requests - a[1].requests)) {
    console.log(`  ${pad(k, 12)}${padL(n(v.requests), 7)}${padL(n(v.input_tokens), 14)}${padL(n(v.output_tokens), 12)}`);
  }
}

if (Object.keys(byTier).length) {
  console.log('\nBy tier');
  console.log(`  ${pad('TIER', 12)}${padL('REQ', 7)}${padL('INPUT', 14)}${padL('OUTPUT', 12)}`);
  for (const [k, v] of Object.entries(byTier).sort((a, b) => b[1].requests - a[1].requests)) {
    console.log(`  ${pad(k, 12)}${padL(n(v.requests), 7)}${padL(n(v.input_tokens), 14)}${padL(n(v.output_tokens), 12)}`);
  }
}
console.log('');
