(() => {
  if (navigator.doNotTrack === "1" || navigator.globalPrivacyControl) return;
  if (document.visibilityState === "prerender") return;
  fetch("/api/visits/track", {
    method: "POST",
    credentials: "omit",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ page: location.pathname, referrer: document.referrer, language: navigator.language }),
    keepalive: true,
  }).catch(() => {});
})();
