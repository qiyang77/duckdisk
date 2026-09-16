import test from 'node:test';
import assert from 'node:assert/strict';
import { scryptSync } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createApp } from './server.mjs';
const salt = '0123456789abcdef0123456789abcdef';
const password = 'test-only-password';
await test('authentication, statistics, privacy, logout and throttling', async t => {
  const app = await createApp({dbPath: ':memory:', secret: 'a'.repeat(64), passwordHash: `${salt}:${scryptSync(password, salt, 64).toString('hex')}`});
  await new Promise(resolve => app.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => app.close(resolve)));
  const base = `http://127.0.0.1:${app.address().port}`;
  const request = (path, data, headers = {}) => fetch(base + path, {method: data ? 'POST' : 'GET', headers: {'Content-Type':'application/json', Origin:'https://duckdisk.com', 'User-Agent':'Mozilla/5.0 Macintosh Safari/605.1', ...headers}, ...(data ? {body: JSON.stringify(data)} : {})});
  assert.equal((await request('/api/admin/visits')).status, 401);
  assert.equal((await request('/api/auth/login', {email:'admin@duckdisk.com',password:'wrong'})).status,401);
  assert.equal((await request('/api/auth/login', {email:'admin@duckdisk.com',password}, {Origin:'https://evil.test'})).status,403);
  const login = await request('/api/auth/login', {email:'admin@duckdisk.com',password});
  assert.equal(login.status,200);
  const setCookie = login.headers.get('set-cookie');
  assert.match(setCookie,/HttpOnly; Secure; SameSite=Strict/);
  const Cookie = setCookie.split(';')[0];
  assert.equal((await request('/api/admin/visits?days=10000',null,{Cookie})).status,400);
  const visit = {page:'/',referrer:'https://example.com/private?secret=removed',language:'en'};
  for(let i=0;i<2;i++) assert.equal((await (await request('/api/visits/track',visit,{'X-Real-IP':'8.8.8.8'})).json()).tracked,true);
  await request('/api/visits/track',visit,{'X-Real-IP':'8.8.8.8','CF-Connecting-IP':'1.1.1.1'});
  assert.equal((await (await request('/api/visits/track',visit,{DNT:'1'})).json()).tracked,false);
  assert.equal((await request('/api/visits/track',{page:'/admin/'})).status,400);
  assert.equal((await request('/api/visits/track',{page:'/?token=secret'})).status,400);
  const data = await (await request('/api/admin/visits?days=7&limit=1',null,{Cookie})).json();
  assert.equal(data.summary.pageviews,3);
  assert.equal(data.summary.visits,1);
  assert.equal(data.events.length,1);
  assert.equal(data.events[0].ip,'8.8.8.8');
  assert.equal(data.events[0].referrer,'https://example.com');
  assert.equal(data.locations[0].pageviews,3);
  for (const days of [7, 31, 180, 365]) {
    const response = await request(`/api/admin/visits?days=${days}`, null, {Cookie});
    assert.equal(response.status, 200);
    const range = await response.json();
    assert.equal(range.daily.length, days);
    assert.equal(range.daily[0].date, range.summary.startDate);
    assert.equal(range.daily.at(-1).date, range.summary.endDate);
    assert.equal(range.daily.at(-1).visitors, 1);
    assert.equal(range.daily.at(-1).pageviews, 3);
    assert.deepEqual(range.daily[0], {date: range.summary.startDate, visitors: 0, pageviews: 0});
  }
  assert.equal((await request('/api/admin/visits?days=90',null,{Cookie})).status,400);
  await request('/api/visits/track',visit,{'X-Real-IP':'2001:db8:abcd:1234:5678:90ab:cdef:1234'});
  const ipv6 = await (await request('/api/admin/visits?days=7', null, {Cookie})).json();
  assert.equal(ipv6.events[0].ip, '2001:db8:abcd:1234:5678:90ab:cdef:1234');
  assert.equal(ipv6.daily.at(-1).visitors, 2);
  await request('/api/auth/logout',{}, {Cookie});
  assert.equal((await request('/api/admin/visits',null,{Cookie})).status,401);
  for(let i=0;i<10;i++) await request('/api/auth/login',{email:'admin@duckdisk.com',password:'wrong'},{'X-Real-IP':'1.2.3.4'});
  assert.equal((await request('/api/auth/login',{email:'admin@duckdisk.com',password},{'X-Real-IP':'1.2.3.4'})).status,429);
});

await test('year-long retention and daily aggregation across range boundaries', async t => {
  const dir = mkdtempSync(join(tmpdir(), 'duckdisk-analytics-test-'));
  t.after(() => rmSync(dir, {recursive:true, force:true}));
  const dbPath = join(dir, 'visits.sqlite');
  const config = {dbPath, secret:'a'.repeat(64), passwordHash:`${salt}:${scryptSync(password, salt, 64).toString('hex')}`};
  const initial = await createApp(config);
  await new Promise(resolve => initial.listen(0, '127.0.0.1', resolve));
  await new Promise(resolve => initial.close(resolve));
  const db = new DatabaseSync(dbPath);
  const today = new Date().toISOString().slice(0,10);
  const dateAgo = n => new Date(Date.parse(today) - n * 86400000).toISOString().slice(0,10);
  const insert = db.prepare('INSERT INTO visits (occurredAt,visitorId,page,referrer,language,ip,location,device) VALUES (?,?,?,?,?,?,?,?)');
  for (const age of [0,6,7,30,31,179,180,364,366]) {
    for (const visitor of ['first','first','second']) insert.run(dateAgo(age)+'T00:00:00.000Z',`${age}-${visitor}`,'/','','en','192.0.2.1','{}','{}');
  }
  db.close();
  const app = await createApp(config);
  await new Promise(resolve => app.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => app.close(resolve)));
  const base = `http://127.0.0.1:${app.address().port}`;
  const login = await fetch(base+'/api/auth/login',{method:'POST',headers:{'Content-Type':'application/json',Origin:'https://duckdisk.com'},body:JSON.stringify({email:'admin@duckdisk.com',password})});
  const Cookie = login.headers.get('set-cookie').split(';')[0];
  for (const [days, activeDays] of [[7,2],[31,4],[180,6],[365,8]]) {
    const result = await (await fetch(base+`/api/admin/visits?days=${days}&limit=1`,{headers:{Cookie}})).json();
    assert.equal(result.daily.length,days);
    assert.equal(result.summary.visits,activeDays*2);
    assert.equal(result.summary.pageviews,activeDays*3);
    assert.equal(result.daily.reduce((sum,day)=>sum+day.visitors,0),result.summary.visits);
    assert.equal(result.daily.filter(day=>day.visitors===2).length,activeDays);
    assert.equal(result.events.length,1); // Chart aggregation is independent of the detail limit.
  }
  const verify = new DatabaseSync(dbPath);
  assert.equal(verify.prepare('SELECT COUNT(*) count FROM visits').get().count,24);
  verify.close();
});
