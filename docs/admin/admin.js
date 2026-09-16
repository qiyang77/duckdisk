(() => {
  const loginView = document.getElementById("login-view");
  const dashboardView = document.getElementById("dashboard-view");
  const loginForm = document.getElementById("login-form");
  const loginMessage = document.getElementById("login-message");
  const rangeSelect = document.getElementById("range-select");
  const refreshButton = document.getElementById("refresh-button");
  const logoutButton = document.getElementById("logout-button");
  const metricPageviews = document.getElementById("metric-pageviews");
  const metricVisitors = document.getElementById("metric-visitors");
  const metricRegions = document.getElementById("metric-regions");
  const metricRange = document.getElementById("metric-range");
  const countryList = document.getElementById("country-list");
  const pageList = document.getElementById("page-list");
  const visitTable = document.getElementById("visit-table");
  const tableNote = document.getElementById("table-note");
  const chartNode = document.getElementById("visitors-chart");
  const chartDetail = document.getElementById("chart-detail");
  let chartSeries = [];
  let selectedDay = 0;
  let chartGeometry;

  let map;
  let markerLayer;

  function escapeHtml(value) {
    return String(value ?? "").replace(/[&<>"']/g, (char) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    }[char]));
  }

  async function api(path, options = {}) {
    const response = await fetch(path, {
      credentials: "same-origin",
      cache: "no-store",
      ...options,
      headers: {
        "content-type": "application/json",
        ...(options.headers || {}),

      },
    });
    const payload = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(payload.message || "请求失败");
      error.status = response.status;
      throw error;
    }
    return payload;
  }

  function showDashboard(show) {
    loginView.hidden = show;
    dashboardView.hidden = !show;
    if (show) {
      document.body.classList.add("is-dashboard");
      if (location.hash !== "#dashboard") history.replaceState(null, "", "#dashboard");
      requestAnimationFrame(() => {
        map?.invalidateSize();
        setTimeout(() => map?.invalidateSize(), 220);
      });
    } else {
      document.body.classList.remove("is-dashboard");
      if (location.hash === "#dashboard") history.replaceState(null, "", location.pathname);
    }
  }

  function formatDateTime(value) {
    if (!value) return "-";
    return new Date(value).toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  function formatDate(value) {
    if (!value) return "-";
    return new Date(`${value}T12:00:00Z`).toLocaleDateString("zh-CN", {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }

  function locationLabel(location) {
    if (!location) return "未知";
    return [location.city, location.region, location.countryZh || location.country]
      .filter(Boolean)
      .join(", ") || "未知";
  }

  function initMap() {
    if (map) return;
    if (typeof L === "undefined") {
      throw new Error("Leaflet 地图资源加载失败");
    }
    map = L.map("visit-map", {
      worldCopyJump: true,
      zoomControl: true,
      preferCanvas: true,
    }).setView([28, 10], 2);
    L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
      maxZoom: 12,
      attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>',
    }).addTo(map);
    markerLayer = L.layerGroup().addTo(map);
  }

  function markerIcon(count) {
    return L.divIcon({
      className: "",
      html: `<span class="marker-dot">${count}</span>`,
      iconSize: [30, 30],
      iconAnchor: [15, 15],
      popupAnchor: [0, -16],
    });
  }

  function renderMap(events) {
    initMap();
    markerLayer.clearLayers();
    const grouped = new Map();
    events.forEach((event) => {
      const loc = event.location || {};
      if (!Number.isFinite(loc.lat) || !Number.isFinite(loc.lng)) return;
      const key = `${loc.lat.toFixed(3)},${loc.lng.toFixed(3)}`;
      const current = grouped.get(key) || { loc, events: [] };
      current.events.push(event);
      current.count = (current.count || 0) + (event.pageviews || 1);
      grouped.set(key, current);
    });

    const bounds = [];
    grouped.forEach(({ loc, events, count }) => {
      const latest = events[0];
      const marker = L.marker([loc.lat, loc.lng], {
        icon: markerIcon(count),
      }).bindPopup(`
        <strong>${escapeHtml(locationLabel(loc))}</strong>
        <br>${count} 次浏览
        ${latest.page ? `<br>${escapeHtml(latest.page)}` : ""}
        <br><small>${escapeHtml(loc.precision || "country")} location</small>
      `);
      marker.addTo(markerLayer);
      bounds.push([loc.lat, loc.lng]);
    });

    if (bounds.length) {
      map.fitBounds(bounds, { padding: [40, 40], maxZoom: 5 });
    } else {
      map.setView([28, 10], 2);
    }
  }

  function renderRankList(node, items, labelKey) {
    node.innerHTML = items.length
      ? items.map((item) => `
          <li>
            <span>${escapeHtml(item[labelKey])}<small>${escapeHtml(item.sub || "")}</small></span>
            <strong>${item.pageviews}</strong>
          </li>
        `).join("")
      : `<li><span>暂无数据</span><strong>0</strong></li>`;
  }

  function renderTable(events) {
    tableNote.textContent = `显示最近 ${events.length} 条访问记录`;
    visitTable.innerHTML = events.length
      ? events.map((event) => {
          const loc = event.location || {};
          const device = event.device || {};
          return `
            <tr>
              <td>${formatDateTime(event.occurredAt)}<small>${escapeHtml(event.visitorId || "")}</small></td>
              <td>${escapeHtml(event.page || "/")}<small>${escapeHtml(event.language || "")}</small></td>
              <td>${escapeHtml(locationLabel(loc))}<small>${escapeHtml(`${loc.lat ?? ""}, ${loc.lng ?? ""} · ${loc.precision || ""}`)}</small></td>
              <td>${escapeHtml(event.ip || "")}<small>${escapeHtml(event.ipHash || "")}</small></td>
              <td>${escapeHtml([device.device, device.os, device.browser].filter(Boolean).join(" / "))}<small>${escapeHtml(event.userAgent || "")}</small></td>
              <td>${escapeHtml(event.referrer || "-")}</td>
            </tr>
          `;
        }).join("")
      : `<tr><td colspan="6">暂无访问记录</td></tr>`;
  }

  function selectChartDay(index) {
    if (!chartSeries.length || !chartGeometry) return;
    selectedDay = Math.max(0, Math.min(chartSeries.length - 1, index));
    const day = chartSeries[selectedDay];
    const { x, y, top, bottom } = chartGeometry;
    const dot = chartNode.querySelector(".chart-selected");
    const guide = chartNode.querySelector(".chart-guide");
    dot.setAttribute("cx", x(selectedDay));
    dot.setAttribute("cy", y(day.visitors));
    guide.setAttribute("d", `M${x(selectedDay)},${top} V${bottom}`);
    chartDetail.textContent = `${day.date} · ${day.visitors.toLocaleString()} 位访客 · ${day.pageviews.toLocaleString()} 次浏览`;
  }

  function drawChart() {
    if (!chartSeries.length || dashboardView.hidden) return;
    const width = Math.max(260, chartNode.clientWidth);
    const height = 248, left = 42, right = width - 16, top = 18, bottom = height - 34;
    const peak = Math.max(...chartSeries.map(day => day.visitors));
    const step = Math.max(1, Math.ceil(peak / 4));
    const max = step * 4;
    const x = index => left + index / Math.max(1, chartSeries.length - 1) * (right - left);
    const y = count => bottom - count / max * (bottom - top);
    chartGeometry = { x, y, left, right, top, bottom };
    const points = chartSeries.map((day, i) => `${x(i)},${y(day.visitors)}`).join(" L");
    const ticks = Array.from({ length: 5 }, (_, i) => {
      const value = step * i;
      return `<line x1="${left}" y1="${y(value)}" x2="${right}" y2="${y(value)}" class="chart-grid"/><text x="${left - 10}" y="${y(value) + 4}" text-anchor="end">${value.toLocaleString()}</text>`;
    }).join("");
    const labelCount = width < 500 ? 3 : 6;
    const labels = Array.from({ length: labelCount }, (_, i) => {
      const index = Math.round(i / (labelCount - 1) * (chartSeries.length - 1));
      return `<text x="${x(index)}" y="${height - 8}" text-anchor="${i === 0 ? "start" : i === labelCount - 1 ? "end" : "middle"}">${escapeHtml(chartSeries[index].date.slice(5).replace("-", "/"))}</text>`;
    }).join("");
    chartNode.innerHTML = `<svg viewBox="0 0 ${width} ${height}" role="img" tabindex="0" aria-label="每日访客数量折线图，左右方向键选择日期">
      <title>${escapeHtml(chartSeries[0].date)} 至 ${escapeHtml(chartSeries.at(-1).date)}，每日最高 ${peak} 位访客</title>
      <defs><linearGradient id="visitor-fill" x1="0" y1="0" x2="0" y2="1"><stop stop-color="#ffd45a" stop-opacity=".2"/><stop offset="1" stop-color="#ffd45a" stop-opacity="0"/></linearGradient></defs>
      ${ticks}${labels}
      <path d="M${left},${bottom} L${points} L${right},${bottom} Z" fill="url(#visitor-fill)"/>
      <path d="M${points}" fill="none" stroke="#ffd45a" stroke-width="2.5" stroke-linejoin="round"/>
      <path class="chart-guide" stroke="#b7bdb0" stroke-dasharray="3 5" fill="none"/>
      <circle class="chart-selected" r="5" fill="#ffd45a" stroke="#1d211b" stroke-width="2"/>
    </svg>`;
    document.getElementById("chart-summary").textContent = `日均 ${(chartSeries.reduce((sum, day) => sum + day.visitors, 0) / chartSeries.length).toFixed(1)} · 单日最高 ${peak.toLocaleString()}`;
    selectChartDay(selectedDay);
  }

  function pointChart(event) {
    if (!chartGeometry) return;
    const bounds = chartNode.getBoundingClientRect();
    const { left, right } = chartGeometry;
    const index = Math.round((event.clientX - bounds.left - left) / (right - left) * (chartSeries.length - 1));
    selectChartDay(index);
  }
  chartNode.addEventListener("pointermove", pointChart);
  chartNode.addEventListener("pointerdown", pointChart);
  chartNode.addEventListener("keydown", event => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    selectChartDay(event.key === "Home" ? 0 : event.key === "End" ? chartSeries.length - 1 : selectedDay + (event.key === "ArrowRight" ? 1 : -1));
  });
  new ResizeObserver(drawChart).observe(chartNode);

  function render(payload) {
    const summary = payload.summary || {};
    const countries = summary.countries || [];
    const events = payload.events || [];
    metricPageviews.textContent = summary.pageviews || 0;
    metricVisitors.textContent = summary.visits || 0;
    metricRegions.textContent = countries.filter(c => c.code !== "XX").length;
    metricRange.textContent = `${formatDate(summary.startDate)} - ${formatDate(summary.endDate)}`;
    chartSeries = payload.daily || [];
    selectedDay = chartSeries.length - 1;
    drawChart();
    renderRankList(countryList, countries.map((country) => ({
      name: country.zh || country.name,
      sub: country.code,
      pageviews: country.pageviews,
    })), "name");
    renderRankList(pageList, (payload.pages || []).map((page) => ({
      page: page.page,
      sub: "",
      pageviews: page.pageviews,
    })), "page");
    try { renderMap(payload.locations || events); } catch {
      document.getElementById("visit-map").textContent = "地图暂时不可用，访问统计仍可查看。";
    }
    renderTable(events);
  }

  async function loadDashboard() {
    const days = rangeSelect.value || "31";
    document.getElementById("dashboard-message").textContent = "";
    const payload = await api(`/api/admin/visits?days=${encodeURIComponent(days)}&limit=300`);
    showDashboard(true);
    await new Promise((resolve) => requestAnimationFrame(resolve));
    render(payload);
    map?.invalidateSize();
    document.getElementById("updated-at").textContent = ` 最近更新：${new Date().toLocaleTimeString("zh-CN")}`;
  }

  loginForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const submit = loginForm.querySelector("button");
    submit.disabled = true;
    submit.textContent = "登录中…";
    loginMessage.textContent = "";
    const form = new FormData(loginForm);
    try {
      const payload = await api("/api/auth/login", {
        method: "POST",
        body: JSON.stringify({
          email: form.get("email"),
          password: form.get("password"),
        }),
      });
      if (payload.user?.role !== "admin") {
        throw new Error("需要管理员账号");
      }

      loginForm.reset();
      await loadDashboard();
    } catch (error) {
      showDashboard(false);
      loginMessage.textContent = error.message || "登录失败";
    } finally { submit.disabled = false; submit.textContent = "登录"; }
  });

  function handleError(error) {
    if (error.status === 401 || error.status === 403) {
      showDashboard(false);
      loginMessage.textContent = "登录已过期，请重新登录。";
    } else {
      const node = dashboardView.hidden ? loginMessage : document.getElementById("dashboard-message");
      node.textContent = error.message || "加载失败，请重试。";
    }
  }
  async function refresh() {
    refreshButton.disabled = true;
    rangeSelect.disabled = true;
    try { await loadDashboard(); } catch (error) { handleError(error); }
    finally { refreshButton.disabled = false; rangeSelect.disabled = false; }
  }
  refreshButton.addEventListener("click", refresh);
  rangeSelect.addEventListener("change", refresh);
  logoutButton.addEventListener("click", async () => {
    try { await api("/api/auth/logout", {method: "POST", body: "{}"}); showDashboard(false); }
    catch (error) { handleError(error); }
  });
  loadDashboard().catch(error => { if (error.status !== 401) handleError(error); });
})();
