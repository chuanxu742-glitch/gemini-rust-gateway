const $ = (id) => document.getElementById(id);

Promise.all([
  chrome.storage.local.get(["gateway", "adminToken", "accountId", "authUser", "weight", "rememberToken", "autoSync", "accounts", "lastAutoSync"]),
  chrome.storage.session.get(["adminToken"])
]).then(([saved, session]) => {
  if (saved.gateway) $("gateway").value = saved.gateway;
  if (saved.accountId) $("account-id").value = saved.accountId;
  if (Number.isInteger(saved.authUser)) $("auth-user").value = saved.authUser;
  if (Number.isInteger(saved.weight)) $("weight").value = saved.weight;
  $("remember-token").checked = Boolean(saved.rememberToken);
  $("auto-sync").checked = Boolean(saved.autoSync);
  $("admin-token").value = saved.adminToken || session.adminToken || "";
  renderAccounts(saved.accounts || [], saved.lastAutoSync);
});

function renderAccounts(accounts, lastSync) {
  const root = $("accounts"); root.replaceChildren();
  for (const account of accounts) {
    const row = document.createElement("div"); row.className = "account";
    const result = lastSync?.results?.find((item) => item.id === account.id);
    const state = result ? (result.ok ? "已同步" : "失败") : "已保存";
    row.innerHTML = `<span></span><small></small>`;
    row.children[0].textContent = `${account.id} · /u/${account.authUser} · ×${account.weight || 1}`;
    row.children[1].textContent = state;
    root.append(row);
  }
}

function localGateway(raw) {
  const url = new URL(raw);
  if (url.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(url.hostname)) {
    throw new Error("网关必须是 http://127.0.0.1 或 http://localhost");
  }
  return url.origin;
}

function accountId(raw) {
  const value = raw.trim();
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(value)) {
    throw new Error("账号名称只能包含字母、数字、点、下划线或连字符");
  }
  return value;
}

async function getCookie(name) {
  const cookies = await chrome.cookies.getAll({ name });
  return cookies.find((cookie) => cookie.domain === ".google.com") ||
    cookies.find((cookie) => cookie.domain.endsWith("google.com"));
}

$("sync").addEventListener("click", async () => {
  const button = $("sync");
  const status = $("status");
  button.disabled = true;
  status.className = "";
  status.textContent = "正在读取并验证 Cookie…";
  try {
    const gateway = localGateway($("gateway").value.trim());
    const adminToken = $("admin-token").value.trim();
    const id = accountId($("account-id").value);
    const authUser = Number($("auth-user").value);
    const weight = Number($("weight").value);
    const autoSync = $("auto-sync").checked;
    const rememberToken = $("remember-token").checked || autoSync;
    if (!adminToken) throw new Error("请填写网关管理令牌");
    if (!Number.isInteger(authUser) || authUser < 0 || authUser > 20) {
      throw new Error("Google 账号序号必须在 0 到 20 之间");
    }
    if (!Number.isInteger(weight) || weight < 1 || weight > 100) {
      throw new Error("轮换权重必须在 1 到 100 之间");
    }
    const [psid, psidts] = await Promise.all([
      getCookie("__Secure-1PSID"),
      getCookie("__Secure-1PSIDTS")
    ]);
    if (!psid?.value) throw new Error("没有找到 __Secure-1PSID，请先登录 Gemini Web");
    const response = await fetch(`${gateway}/admin/cookies/sync`, {
      method: "POST",
      headers: { "Authorization": `Bearer ${adminToken}`, "Content-Type": "application/json" },
      body: JSON.stringify({
        account_id: id,
        auth_user: authUser,
        weight,
        secure_1psid: psid.value,
        secure_1psidts: psidts?.value || null
      })
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data?.error?.message || `${response.status} ${response.statusText}`);

    const saved = await chrome.storage.local.get("accounts");
    const accounts = (saved.accounts || []).filter((account) => account.id !== id);
    accounts.push({ id, authUser, weight }); accounts.sort((a, b) => a.id.localeCompare(b.id));
    await chrome.storage.local.set({ gateway, accountId: id, authUser, weight, rememberToken, autoSync, accounts });
    if (rememberToken) {
      await chrome.storage.local.set({ adminToken });
      await chrome.storage.session.remove("adminToken");
    } else {
      await chrome.storage.local.remove("adminToken");
      await chrome.storage.session.set({ adminToken });
    }
    status.className = "ok";
    status.textContent = `账号 ${id}（/u/${authUser}）已验证并加入轮换池。`;
    renderAccounts(accounts);
  } catch (error) {
    status.className = "error";
    status.textContent = error.message;
  } finally {
    button.disabled = false;
  }
});

$("sync-all").addEventListener("click", async () => {
  const status = $("status");
  status.className = ""; status.textContent = "正在同步全部已保存账号…";
  const response = await chrome.runtime.sendMessage({ type: "sync-all" });
  if (!response?.ok) {
    status.className = "error"; status.textContent = response?.error || "同步失败"; return;
  }
  const saved = await chrome.storage.local.get(["accounts", "lastAutoSync"]);
  renderAccounts(saved.accounts || [], saved.lastAutoSync);
  status.className = "ok"; status.textContent = "全部账号同步完成。";
});
