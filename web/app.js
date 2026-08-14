(() => {
  "use strict";
  const $ = (selector) => document.querySelector(selector);
  const $$ = (selector) => [...document.querySelectorAll(selector)];
  const terminalStates = new Set(["completed", "failed", "interrupted", "rejected", "canceled"]);
  const state = { messages: [], polling: null };

  const baseUrlInput = $("#base-url");
  const apiKeyInput = $("#api-key");
  const rememberInput = $("#remember-key");
  const accountInput = $("#account-select");
  const adminTokenInput = $("#admin-token");
  baseUrlInput.value = localStorage.getItem("gateway.baseUrl") || location.origin;
  apiKeyInput.value = localStorage.getItem("gateway.apiKey") || sessionStorage.getItem("gateway.apiKey") || "";
  rememberInput.checked = Boolean(localStorage.getItem("gateway.apiKey"));
  accountInput.value = localStorage.getItem("gateway.accountId") || "";
  adminTokenInput.value = sessionStorage.getItem("gateway.adminToken") || "";

  function baseUrl() { return baseUrlInput.value.trim().replace(/\/$/, "") || location.origin; }
  function apiKey() { return apiKeyInput.value.trim(); }
  function authHeaders() {
    const headers = apiKey() ? { Authorization: `Bearer ${apiKey()}` } : {};
    if (accountInput.value) headers["X-Gemini-Account"] = accountInput.value;
    return headers;
  }
  function toast(message, error = false) {
    const node = $("#toast"); node.textContent = message; node.className = `toast show${error ? " error" : ""}`;
    clearTimeout(toast.timer); toast.timer = setTimeout(() => node.className = "toast", 3200);
  }
  async function readError(response) {
    try { const data = await response.json(); return data?.error?.message || data?.message || JSON.stringify(data); }
    catch { return `${response.status} ${response.statusText}`; }
  }
  async function request(path, options = {}) {
    const headers = { ...authHeaders(), ...(options.body ? { "Content-Type": "application/json" } : {}), ...options.headers };
    const response = await fetch(`${baseUrl()}${path}`, { ...options, headers });
    if (!response.ok) throw new Error(await readError(response));
    return response;
  }
  async function requestAdmin(path, options = {}) {
    const token = adminTokenInput.value.trim();
    if (!token) throw new Error("请先在连接设置中填写管理令牌");
    return request(path, { ...options, headers: { ...options.headers, Authorization: `Bearer ${token}` } });
  }
  function setService(text, kind = "") { const node = $("#service-state"); node.className = `status ${kind}`; node.innerHTML = "<i></i>"; node.append(document.createTextNode(text)); }
  function populateModels(data) {
    const models = data?.data?.map((item) => item.id) || [];
    for (const select of [$("#chat-model"), $("#image-model")]) {
      const selected = select.value; select.replaceChildren();
      for (const model of models) { const option = document.createElement("option"); option.value = model; option.textContent = model; select.append(option); }
      if (models.includes(selected)) select.value = selected;
    }
  }
  function populateAccounts(data) {
    const selected = localStorage.getItem("gateway.accountId") || accountInput.value;
    accountInput.replaceChildren();
    const automatic = document.createElement("option"); automatic.value = ""; automatic.textContent = "自动轮换"; accountInput.append(automatic);
    for (const account of data?.data || []) {
      const option = document.createElement("option"); option.value = account.id;
      const state = account.ready ? (account.cooldown_seconds ? `冷却 ${account.cooldown_seconds}s` : "可用") : "未就绪";
      option.textContent = `${account.id} · /u/${account.auth_user} · ${state}`; option.disabled = !account.ready;
      accountInput.append(option);
    }
    if ([...accountInput.options].some((option) => option.value === selected && !option.disabled)) accountInput.value = selected;
    renderAccounts(data?.data || []);
  }
  function renderAccounts(accounts) {
    const root = $("#account-list"); root.replaceChildren();
    if (!accounts.length) { root.innerHTML = '<div class="task-result empty">尚未同步账号</div>'; return; }
    for (const account of accounts) {
      const card = document.createElement("article"); card.className = "account-card";
      const info = document.createElement("div"); const title = document.createElement("h3"); title.textContent = account.id;
      const detail = document.createElement("p"); detail.textContent = `/u/${account.auth_user} · ${account.ready ? "就绪" : "未就绪"} · ${account.inflight} 个请求 · 冷却 ${account.cooldown_seconds}s`;
      info.append(title, detail);
      const weight = document.createElement("input"); weight.type = "number"; weight.min = "1"; weight.max = "100"; weight.value = account.weight || 1; weight.ariaLabel = `${account.id} 权重`;
      const toggle = document.createElement("button"); toggle.className = "button ghost"; toggle.type = "button"; toggle.textContent = account.enabled ? "暂停" : "启用";
      const actions = document.createElement("div"); actions.className = "account-actions";
      const save = document.createElement("button"); save.className = "button primary"; save.type = "button"; save.textContent = "保存";
      const remove = document.createElement("button"); remove.className = "button danger"; remove.type = "button"; remove.textContent = "删除";
      save.addEventListener("click", async () => {
        try { await requestAdmin(`/admin/accounts/${encodeURIComponent(account.id)}`, { method: "PATCH", body: JSON.stringify({ weight: Number(weight.value) }) }); await loadAccounts(); toast("账号权重已更新"); }
        catch (error) { toast(error.message, true); }
      });
      toggle.addEventListener("click", async () => {
        try { await requestAdmin(`/admin/accounts/${encodeURIComponent(account.id)}`, { method: "PATCH", body: JSON.stringify({ enabled: !account.enabled }) }); await loadAccounts(); }
        catch (error) { toast(error.message, true); }
      });
      remove.addEventListener("click", async () => {
        if (!confirm(`确定删除账号“${account.id}”及其本地凭证吗？`)) return;
        try { await requestAdmin(`/admin/accounts/${encodeURIComponent(account.id)}`, { method: "DELETE" }); await loadAccounts(); toast("账号已删除"); }
        catch (error) { toast(error.message, true); }
      });
      actions.append(save, remove); card.append(info, weight, toggle, actions); root.append(card);
    }
  }
  async function loadAccounts() {
    const data = await (await request("/v1/accounts")).json(); populateAccounts(data); return data;
  }
  async function checkConnection() {
    setService("正在检测");
    try {
      await request("/health", { headers: {} });
      if (!apiKey()) { setService("服务在线 · 待填写 Key"); $("#settings").classList.add("open"); return; }
      const [modelsResponse, capsResponse, accountsResponse] = await Promise.all([request("/v1/models"), request("/v1/capabilities"), request("/v1/accounts")]);
      const models = await modelsResponse.json(); const caps = await capsResponse.json(); const accounts = await accountsResponse.json(); populateModels(models); populateAccounts(accounts);
      const enabled = Object.entries(caps.generation_features || {}).filter(([, value]) => value.enabled).map(([name]) => name);
      $("#capability-summary").textContent = `流式聊天 · ${models.data?.length || 0} 个模型 · ${enabled.length} 项生成能力`;
      setService("服务与认证正常", "ok");
    } catch (error) { setService("连接失败", "error"); toast(error.message, true); }
  }

  $("#settings-button").addEventListener("click", () => $("#settings").classList.toggle("open"));
  $("#save-settings").addEventListener("click", () => {
    localStorage.setItem("gateway.baseUrl", baseUrl());
    localStorage.setItem("gateway.accountId", accountInput.value);
    sessionStorage.setItem("gateway.adminToken", adminTokenInput.value.trim());
    localStorage.removeItem("gateway.apiKey"); sessionStorage.removeItem("gateway.apiKey");
    (rememberInput.checked ? localStorage : sessionStorage).setItem("gateway.apiKey", apiKey());
    checkConnection();
  });
  $$(".tab").forEach((tab) => tab.addEventListener("click", () => {
    $$(".tab").forEach((item) => item.classList.toggle("active", item === tab));
    $$(".panel").forEach((panel) => panel.classList.toggle("active", panel.id === `panel-${tab.dataset.panel}`));
  }));

  function removeEmpty() { $("#messages .empty-state")?.remove(); }
  function addMessage(role, content = "", isError = false) {
    removeEmpty(); const row = document.createElement("div"); row.className = `message ${role}${isError ? " error" : ""}`;
    const avatar = document.createElement("div"); avatar.className = "avatar"; avatar.textContent = role === "user" ? "你" : "AI";
    const bubble = document.createElement("div"); bubble.className = "bubble"; bubble.textContent = content;
    row.append(avatar, bubble); $("#messages").append(row); $("#messages").scrollTop = $("#messages").scrollHeight;
    return bubble;
  }
  async function sendChat(prompt) {
    state.messages.push({ role: "user", content: prompt }); addMessage("user", prompt);
    const output = addMessage("assistant"); const button = $("#send-chat"); button.disabled = true;
    try {
      const response = await request("/v1/chat/completions", { method: "POST", body: JSON.stringify({ model: $("#chat-model").value, messages: state.messages, stream: true }) });
      if (!response.body) throw new Error("浏览器没有收到流式响应");
      const reader = response.body.getReader(); const decoder = new TextDecoder(); let buffer = ""; let text = "";
      while (true) {
        const { value, done } = await reader.read(); buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
        const frames = buffer.split(/\r?\n\r?\n/); buffer = frames.pop() || "";
        for (const frame of frames) for (const line of frame.split(/\r?\n/)) {
          if (!line.startsWith("data:")) continue; const raw = line.slice(5).trim(); if (!raw || raw === "[DONE]") continue;
          const data = JSON.parse(raw); if (data.error) throw new Error(data.error.message || "Gemini 返回错误");
          text += data.choices?.[0]?.delta?.content || ""; output.textContent = text; $("#messages").scrollTop = $("#messages").scrollHeight;
        }
        if (done) break;
      }
      if (!text) output.textContent = "（模型没有返回文本）"; state.messages.push({ role: "assistant", content: text });
    } catch (error) { output.textContent = `请求失败：${error.message}`; output.closest(".message").classList.add("error"); }
    finally { button.disabled = false; $("#chat-input").focus(); }
  }
  $("#chat-form").addEventListener("submit", (event) => { event.preventDefault(); const input = $("#chat-input"); const prompt = input.value.trim(); if (prompt) { input.value = ""; sendChat(prompt); } });
  $("#chat-input").addEventListener("keydown", (event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); $("#chat-form").requestSubmit(); } });
  $("#clear-chat").addEventListener("click", () => { state.messages = []; $("#messages").innerHTML = '<div class="empty-state"><span>✦</span><h2>新对话</h2><p>上下文已清空。</p></div>'; });

  function fileUrl(url) { try { return new URL(url, `${baseUrl()}/`).href; } catch { return url; } }
  function renderGallery(task) {
    const gallery = $("#image-gallery"); gallery.replaceChildren();
    for (const file of task.result?.files || []) if ((file.content_type || "").startsWith("image/")) {
      const link = document.createElement("a"); link.href = fileUrl(file.url); link.target = "_blank"; link.rel = "noopener";
      const image = document.createElement("img"); image.src = link.href; image.alt = "Gemini 生成的图片"; image.loading = "lazy"; link.append(image); gallery.append(link);
    }
  }
  function renderProgress(task) {
    const box = $("#image-progress"); box.classList.remove("hidden"); box.replaceChildren();
    const left = document.createElement("div"); const title = document.createElement("strong"); title.textContent = task.status === "completed" ? "生成完成" : task.status === "failed" ? "生成失败" : "任务处理中";
    const meta = document.createElement("div"); meta.className = "task-meta"; meta.textContent = task.id; left.append(title, meta);
    const pill = document.createElement("span"); pill.className = "pill"; pill.textContent = task.status; box.append(left, pill);
    if (task.error) { const error = document.createElement("div"); error.className = "task-meta"; error.textContent = task.error; box.append(error); }
  }
  async function pollTask(id, onUpdate) {
    clearTimeout(state.polling);
    try { const task = await (await request(`/v1/tasks/${encodeURIComponent(id)}`)).json(); onUpdate(task); if (!terminalStates.has(task.status)) state.polling = setTimeout(() => pollTask(id, onUpdate), 2500); }
    catch (error) { toast(`任务查询失败：${error.message}`, true); }
  }
  $("#image-form").addEventListener("submit", async (event) => {
    event.preventDefault(); const button = $("#generate-image"); button.disabled = true; $("#image-gallery").replaceChildren();
    try {
      const task = await (await request("/v1/generations", { method: "POST", body: JSON.stringify({ type: "image", prompt: $("#image-prompt").value.trim(), model: $("#image-model").value, count: Number($("#image-count").value) }) })).json();
      renderProgress(task); $("#task-id").value = task.id; pollTask(task.id, (updated) => { renderProgress(updated); if (updated.status === "completed") renderGallery(updated); if (terminalStates.has(updated.status)) button.disabled = false; });
    } catch (error) { toast(error.message, true); button.disabled = false; }
  });

  function renderTask(task) {
    const box = $("#task-result"); box.className = "task-result"; box.replaceChildren();
    const actions = document.createElement("div"); actions.className = "task-actions"; const title = document.createElement("strong"); title.textContent = `${task.kind} · ${task.status}`;
    const cancel = document.createElement("button"); cancel.className = "button ghost"; cancel.type = "button"; cancel.textContent = "取消任务"; cancel.disabled = terminalStates.has(task.status);
    cancel.addEventListener("click", async () => { try { renderTask(await (await request(`/v1/tasks/${encodeURIComponent(task.id)}`, { method: "DELETE" })).json()); } catch (error) { toast(error.message, true); } });
    actions.append(title, cancel); const pre = document.createElement("pre"); pre.textContent = JSON.stringify(task, null, 2); box.append(actions, pre);
  }
  $("#task-form").addEventListener("submit", async (event) => { event.preventDefault(); try { renderTask(await (await request(`/v1/tasks/${encodeURIComponent($("#task-id").value.trim())}`)).json()); } catch (error) { toast(error.message, true); } });
  $("#recent-tasks").addEventListener("click", async () => {
    try { const data = await (await request("/v1/tasks?limit=20")).json(); const box = $("#task-result"); box.className = "task-result"; box.innerHTML = `<pre></pre>`; box.querySelector("pre").textContent = JSON.stringify(data, null, 2); }
    catch (error) { toast(error.message, true); }
  });
  $("#refresh-accounts").addEventListener("click", () => loadAccounts().catch((error) => toast(error.message, true)));

  checkConnection();
})();
