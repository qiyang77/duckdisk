import { createPrivateKey, sign } from "node:crypto";

// Read-only release preflight and post-submission verification. Never print
// the signing key, JWT, account credentials, or App Review contact details.
const keyId = process.env.APP_STORE_CONNECT_KEY_ID;
const issuerId = process.env.APP_STORE_CONNECT_ISSUER_ID;
const privateKey = process.env.APP_STORE_CONNECT_PRIVATE_KEY;
if (!keyId || !issuerId || !privateKey) {
  throw new Error("App Store Connect API credentials are required");
}

const key = createPrivateKey(privateKey);
const now = Math.floor(Date.now() / 1000);
const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
const signingInput = `${encode({ alg: "ES256", kid: keyId, typ: "JWT" })}.${encode({
  iss: issuerId, iat: now, exp: now + 1200, aud: "appstoreconnect-v1",
})}`;
const token = `${signingInput}.${sign("sha256", Buffer.from(signingInput), {
  key, dsaEncoding: "ieee-p1363",
}).toString("base64url")}`;

async function get(path, parameters = {}) {
  const url = new URL(path, "https://api.appstoreconnect.apple.com");
  url.search = new URLSearchParams(parameters).toString();
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  const result = await response.json();
  if (!response.ok) {
    throw new Error(`App Store Connect GET ${path}: HTTP ${response.status}: ${
      result.errors?.map((error) => error.detail || error.title).join("; ") || "Request failed"
    }`);
  }
  return result;
}

const appResponse = await get("/v1/apps", {
  "filter[bundleId]": "com.duckdisk.app", limit: "1",
});
const app = appResponse.data[0];
if (!app) throw new Error("DuckDisk Mac App Store app was not found");

const versions = await get(`/v1/apps/${app.id}/appStoreVersions`, {
  "filter[platform]": "MAC_OS", limit: "25",
});
const builds = await get("/v1/builds", {
  "filter[app]": app.id, sort: "-uploadedDate", limit: "25",
  include: "preReleaseVersion",
});
const prerelease = new Map((builds.included || []).map((item) => [item.id, item.attributes.version]));
console.log(JSON.stringify({
  appId: app.id,
  versions: versions.data.map(({ id, attributes }) => ({
    id, version: attributes.versionString,
    state: attributes.appStoreState || attributes.appVersionState,
    releaseType: attributes.releaseType,
  })),
  builds: builds.data.map(({ id, attributes, relationships }) => ({
    id, build: attributes.version,
    version: prerelease.get(relationships.preReleaseVersion?.data?.id),
    state: attributes.processingState, expired: attributes.expired,
    uploadedDate: attributes.uploadedDate,
  })),
}, null, 2));
