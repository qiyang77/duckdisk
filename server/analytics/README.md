# DuckDisk visitor analytics

The static administrator interface is in `docs/admin/`. Public pages use
`docs/visit-tracker.js`. This independent Node 24 service listens on loopback
port 18791 and uses SQLite outside the static release directories.

Production configuration:

- Service: `duckdisk-analytics.service`, user `duckanalytics`
- Code: `/opt/duckdisk-analytics/`
- Root-only environment file: `/etc/duckdisk-analytics.env`
- SQLite and local GeoIP database: `/var/lib/duckdisk-analytics/`
- Nginx: `/etc/nginx/sites-available/duckdisk.com`
- Previous Nginx configuration: `/etc/nginx/duckdisk.pre-admin-20260917.conf`

Required environment keys are `ADMIN_EMAIL`, `ADMIN_PASSWORD_HASH`,
`ANALYTICS_SECRET`, and `GEOIP_DB`. Password hashes use a 16-byte hex salt and
64-byte scrypt result separated by a colon (Node default scrypt parameters).
Never put credentials or production data in the repository or `docs/`.

Run `npm ci` and `npm test` in this directory. Deploy server changes separately
with SSH/SCP to `/opt/duckdisk-analytics/`, run `npm ci --omit=dev`, and restart
`duckdisk-analytics`. The existing website workflow deploys `docs/` atomically;
backend data persists across frontend deployments and service restarts.

GeoIP uses a separate copy of the server's DB-IP City Lite database, initially
from May 2026. Replace `geoip.mmdb` with a newer DB-IP Lite database and restart
the service to update location accuracy. Attribution appears in the admin UI.
Cloudflare ranges come from https://www.cloudflare.com/ips-v4 and ips-v6.
Only Cloudflare source addresses may supply CF-Connecting-IP. Nginx must always
overwrite X-Real-IP with the connecting peer address; keep the API loopback-only.

The tracking endpoint accepts only known public pages and same-origin requests.
It excludes known bots and DNT/GPC requests. No analytics cookies or browser
storage are used. Full IP addresses are retained in administrator-only records, referrers contain only origins,
and pseudonymous visitor identifiers rotate each UTC day. The visitor metric
is therefore a sum of daily estimated unique visitors, not cross-day people.
All records expire after 365 days (hourly cleanup). Maps and ranks use the full
selected period; the detail table shows the latest 300 visits. Authentication
uses 12-hour Secure/HttpOnly/SameSite cookies, revocable server-side sessions,
scrypt password verification, and login throttling.

The admin supports 7/31/180/365-day ranges and a zero-filled UTC daily visitor
series. Daily visitors are deduplicated independently of pageviews. Older
masked IPs cannot be recovered; full IP retention applies to new visits.
