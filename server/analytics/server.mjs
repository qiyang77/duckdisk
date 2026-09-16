import http from 'node:http';
import { DatabaseSync } from 'node:sqlite';
import { randomBytes, createHash, createHmac, scrypt, timingSafeEqual } from 'node:crypto';
import { promisify } from 'node:util';
import { readFileSync } from 'node:fs';
import { BlockList, isIP } from 'node:net';
import maxmind from 'maxmind';

const derive = promisify(scrypt);
const DAY = 86400000;
const sha = value => createHash('sha256').update(value).digest('hex');
const clip = (value, n) => typeof value === 'string' ? value.slice(0, n) : '';
const allowedPages = new Set(['/', '/index.html', '/privacy.html', '/terms.html']);
const names = new Intl.DisplayNames(['zh-CN'], { type: 'region' });
const countries = JSON.parse(readFileSync(new URL('./countries.json', import.meta.url)));
const cloudflare = new BlockList();
for (const range of JSON.parse(readFileSync(new URL('./cloudflare-ips.json', import.meta.url)))) {
  const [ip, bits] = range.split('/');
  cloudflare.addSubnet(ip, Number(bits), isIP(ip) === 6 ? 'ipv6' : 'ipv4');
}
function clientIP(req) {
  // Nginx overwrites X-Real-IP with the actual connecting address. Trust CF only from CF networks.
  const peer = clip(req.headers['x-real-ip'], 80) || req.socket.remoteAddress;
  const cf = clip(req.headers['cf-connecting-ip'], 80);
  if (isIP(peer) && cloudflare.check(peer, isIP(peer) === 6 ? 'ipv6' : 'ipv4') && isIP(cf)) return cf;
  return isIP(peer) ? peer : 'unknown';
}
function device(ua) {
  const browser = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Other';
  const os = /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iOS' : /Macintosh/.test(ua) ? 'macOS' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : 'Other';
  return { browser, os, device: /iPad|Tablet/.test(ua) ? 'Tablet' : /Mobile|Android|iPhone/.test(ua) ? 'Mobile' : 'Desktop' };
}
function locationFor(reader, ip) {
  const record = reader?.get(ip);
  const code = record?.country?.iso_code || 'XX';
  const meta = code === 'XX' ? null : countries[code];
  const lat = record?.location?.latitude ?? meta?.lat ?? null;
  const lng = record?.location?.longitude ?? meta?.lng ?? null;
  const city = record?.city?.names?.en || '';
  const region = record?.subdivisions?.[0]?.names?.en || '';
  return { countryCode: code, country: record?.country?.names?.en || meta?.name || 'Unknown', countryZh: code === 'XX' ? '未知' : names.of(code), city, region, lat, lng, precision: city ? 'city' : region ? 'region' : code === 'XX' ? 'unknown' : 'country' };
}

export async function createApp(config) {
  const { dbPath, passwordHash, secret, email = 'admin@duckdisk.com', origins = ['https://duckdisk.com', 'https://www.duckdisk.com'], geoPath } = config;
  if (!passwordHash || !secret || secret.length < 32) throw new Error('Admin credentials and analytics secret must be configured');
  const [salt, expectedHex] = passwordHash.split(':');
  if (!/^[a-f0-9]{32}$/.test(salt) || !/^[a-f0-9]{128}$/.test(expectedHex)) throw new Error('Invalid password hash');
  const reader = geoPath ? await maxmind.open(geoPath) : null;
  const db = new DatabaseSync(dbPath);
  db.exec(`PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
    CREATE TABLE IF NOT EXISTS visits (id INTEGER PRIMARY KEY, occurredAt TEXT NOT NULL, visitorId TEXT NOT NULL, page TEXT NOT NULL, referrer TEXT NOT NULL, language TEXT NOT NULL, ip TEXT NOT NULL, location TEXT NOT NULL, device TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS visits_time ON visits(occurredAt);
    CREATE TABLE IF NOT EXISTS sessions (token TEXT PRIMARY KEY, expires INTEGER NOT NULL);
  `);
  function prune() {
    db.prepare('DELETE FROM visits WHERE occurredAt < ?').run(new Date(Date.now() - 365 * DAY).toISOString());
    db.prepare('DELETE FROM sessions WHERE expires < ?').run(Date.now());
  }
  prune();
  const timer = setInterval(prune, 3600000).unref();
  const limits = new Map();
  function limited(key, maximum, duration) {
    const now = Date.now();
    for (const [k, value] of limits) if (value.until < now) limits.delete(k);
    const item = limits.get(key) || { count: 0, until: now + duration };
    item.count++;
    limits.set(key, item);
    return item.count > maximum;
  }
  function session(req) {
    const raw = (req.headers.cookie || '').split(';').map(s => s.trim()).find(s => s.startsWith('__Host-duckdisk_admin='))?.split('=')[1] || '';
    const key = sha(raw);
    return db.prepare('SELECT token FROM sessions WHERE token = ? AND expires > ?').get(key, Date.now())?.token;
  }
  function cookie(value, maxAge) {
    return `__Host-duckdisk_admin=${value}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=${maxAge}`;
  }
  function respond(res, status, payload, headers = {}) {
    res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', ...headers });
    res.end(JSON.stringify(payload));
  }
  async function body(req) {
    if (!/^application\/json\b/.test(req.headers['content-type'] || '')) throw { status: 415, message: '需要 JSON 请求' };
    let raw = '';
    for await (const chunk of req) {
      raw += chunk;
      if (Buffer.byteLength(raw) > 4096) throw { status: 413, message: '请求过大' };
    }
    try {
      const parsed = JSON.parse(raw);
      if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') throw Error();
      return parsed;
    } catch { throw { status: 400, message: '请求格式错误' }; }
  }
  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, 'http://localhost');
      if (req.method === 'GET' && url.pathname === '/api/health') return respond(res, 200, { ok: true });
      if (req.method === 'POST') {
        if (!origins.includes(req.headers.origin)) return respond(res, 403, { message: '请求来源无效' });
        const ip = clientIP(req);
        if (url.pathname === '/api/auth/login') {
          if (limited(`login:${ip}`, 10, 15 * 60000) || limited('login:all', 100, 60000)) return respond(res, 429, { message: '登录尝试过多，请稍后重试' }, { 'Retry-After': '900' });
          const data = await body(req);
          const actual = await derive(clip(data.password, 256), salt, 64);
          if (!timingSafeEqual(actual, Buffer.from(expectedHex, 'hex')) || clip(data.email, 254).trim().toLowerCase() !== email) return respond(res, 401, { message: '邮箱或密码不正确' });
          const token = randomBytes(32).toString('hex');
          const previous = session(req);
          if (previous) db.prepare('DELETE FROM sessions WHERE token = ?').run(previous);
          db.prepare('INSERT INTO sessions VALUES (?, ?)').run(sha(token), Date.now() + 12 * 3600000);
          return respond(res, 200, { user: { email, role: 'admin' } }, { 'Set-Cookie': cookie(token, 43200) });
        }
        if (url.pathname === '/api/auth/logout') {
          const key = session(req);
          if (key) db.prepare('DELETE FROM sessions WHERE token = ?').run(key);
          return respond(res, 200, { ok: true }, { 'Set-Cookie': cookie('', 0) });
        }
        if (url.pathname === '/api/visits/track') {
          if (limited(`track:${ip}`, 90, 60000)) return respond(res, 429, { message: '请求过多' });
          const data = await body(req);
          if (!allowedPages.has(data.page)) return respond(res, 400, { message: '页面无效' });
          const ua = clip(req.headers['user-agent'], 512);
          if (req.headers.dnt === '1' || req.headers['sec-gpc'] === '1' || /bot|crawler|spider|headless|curl|wget/i.test(ua)) return respond(res, 200, { ok: true, tracked: false });
          const now = new Date().toISOString();
          const visitorId = createHmac('sha256', secret).update(`${now.slice(0, 10)}:${ip}:${ua}`).digest('hex').slice(0, 24);
          let referrer = '';
          try { const ref = new URL(clip(data.referrer, 2048)); if (['https:', 'http:'].includes(ref.protocol)) referrer = ref.origin; } catch {}
          db.prepare('INSERT INTO visits (occurredAt,visitorId,page,referrer,language,ip,location,device) VALUES (?,?,?,?,?,?,?,?)').run(now, visitorId, data.page === '/index.html' ? '/' : data.page, referrer, clip(data.language, 32), ip, JSON.stringify(locationFor(reader, ip)), JSON.stringify(device(ua)));
          return respond(res, 200, { ok: true, tracked: true });
        }
      }
      if (req.method === 'GET' && url.pathname === '/api/admin/visits') {
        if (!session(req)) return respond(res, 401, { message: '请先登录' });
        const days = Number(url.searchParams.get('days') || 31);
        const limit = Number(url.searchParams.get('limit') || 300);
        if (![7, 31, 180, 365].includes(days) || !Number.isInteger(limit) || limit < 1 || limit > 1000) return respond(res, 400, { message: '统计范围无效' });
        const endDate = new Date().toISOString().slice(0, 10);
        const startDate = new Date(Date.parse(endDate) - (days - 1) * DAY).toISOString().slice(0, 10);
        const start = startDate + 'T00:00:00.000Z';
        const totals = db.prepare('SELECT COUNT(*) pageviews, COUNT(DISTINCT visitorId) visits FROM visits WHERE occurredAt >= ?').get(start);
        const dailyRows = db.prepare('SELECT substr(occurredAt,1,10) date, COUNT(DISTINCT visitorId) visitors, COUNT(*) pageviews FROM visits WHERE occurredAt >= ? GROUP BY date ORDER BY date').all(start);
        const dailyByDate = new Map(dailyRows.map(row => [row.date, row]));
        const daily = Array.from({ length: days }, (_, index) => {
          const date = new Date(Date.parse(startDate) + index * DAY).toISOString().slice(0, 10);
          return dailyByDate.get(date) || { date, visitors: 0, pageviews: 0 };
        });
        const pages = db.prepare('SELECT page, COUNT(*) pageviews FROM visits WHERE occurredAt >= ? GROUP BY page ORDER BY pageviews DESC').all(start);
        const regions = db.prepare(`SELECT json_extract(location,'$.countryCode') code, json_extract(location,'$.country') name, json_extract(location,'$.countryZh') zh, COUNT(*) pageviews FROM visits WHERE occurredAt >= ? GROUP BY code ORDER BY pageviews DESC`).all(start);
        const locations = db.prepare('SELECT location, COUNT(*) pageviews FROM visits WHERE occurredAt >= ? GROUP BY location').all(start).map(row => ({ ...row, location: JSON.parse(row.location) }));
        const events = db.prepare('SELECT * FROM visits WHERE occurredAt >= ? ORDER BY id DESC LIMIT ?').all(start, limit).map(row => ({ ...row, location: JSON.parse(row.location), device: JSON.parse(row.device) }));
        return respond(res, 200, { summary: { ...totals, countries: regions, startDate, endDate }, pages, events, locations, daily });
      }
      respond(res, 404, { message: '接口不存在' });
    } catch (error) {
      if (!error.status) console.error('Analytics request failed:', error.message);
      respond(res, error.status || 500, { message: error.status ? error.message : '服务暂时不可用' });
    }
  });
  server.requestTimeout = 10000;
  server.headersTimeout = 10000;
  server.on('close', () => { clearInterval(timer); db.close(); });
  return server;
}
if (process.argv[1] === new URL(import.meta.url).pathname) {
  const server = await createApp({ dbPath: process.env.DB_PATH || '/var/lib/duckdisk-analytics/visits.sqlite', passwordHash: process.env.ADMIN_PASSWORD_HASH, secret: process.env.ANALYTICS_SECRET, geoPath: process.env.GEOIP_DB, email: process.env.ADMIN_EMAIL });
  server.listen(Number(process.env.PORT || 18791), '127.0.0.1', () => console.log('DuckDisk analytics ready'));
  process.on('SIGTERM', () => server.close());
}
