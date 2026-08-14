const ALARM = "gemini-rust-cookie-sync";

function localGateway(raw) {
  const url = new URL(raw || "http://127.0.0.1:8000");
  if (url.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(url.hostname)) {
    throw new Error("gateway must be localhost");
  }
  return url.origin;
}

async function cookie(name) {
  const values = await chrome.cookies.getAll({ name });
  return values.find((item) => item.domain === ".google.com") ||
    values.find((item) => item.domain.endsWith("google.com"));
}

async function syncAll(force = false) {
  const [saved, session] = await Promise.all([
    chrome.storage.local.get(["gateway", "adminToken", "autoSync", "accounts"]),
    chrome.storage.session.get("adminToken")
  ]);
  saved.adminToken ||= session.adminToken;
  if ((!saved.autoSync && !force) || !saved.adminToken || !saved.accounts?.length) return;
  const gateway = localGateway(saved.gateway);
  const [psid, psidts] = await Promise.all([
    cookie("__Secure-1PSID"), cookie("__Secure-1PSIDTS")
  ]);
  if (!psid?.value) throw new Error("Gemini Cookie is unavailable");
  const results = [];
  for (const account of saved.accounts) {
    try {
      let response;
      try {
        response = await fetch(`${gateway}/admin/cookies/sync`, {
          method: "POST",
          headers: {
            "Authorization": `Bearer ${saved.adminToken}`,
            "Content-Type": "application/json"
          },
          body: JSON.stringify({
            account_id: account.id,
            auth_user: account.authUser,
            weight: account.weight || 1,
            secure_1psid: psid.value,
            secure_1psidts: psidts?.value || null
          })
        });
      } catch {
        throw new Error(`无法连接 ${gateway}，网关可能未运行`);
      }
      if (response.status === 401) throw new Error("管理令牌不正确");
      if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
      results.push({ id: account.id, ok: true });
    } catch (error) {
      results.push({ id: account.id, ok: false, error: error.message });
    }
  }
  await chrome.storage.local.set({
    lastAutoSync: { at: Date.now(), results }
  });
}

async function configureAlarm() {
  const { autoSync } = await chrome.storage.local.get("autoSync");
  await chrome.alarms.clear(ALARM);
  if (autoSync) {
    await chrome.alarms.create(ALARM, { delayInMinutes: 1, periodInMinutes: 30 });
  }
}

chrome.runtime.onInstalled.addListener(configureAlarm);
chrome.runtime.onStartup.addListener(configureAlarm);
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === ALARM) syncAll().catch(() => {});
});
chrome.storage.onChanged.addListener((changes, area) => {
  if (area === "local" && changes.autoSync) configureAlarm().catch(() => {});
});
chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type !== "sync-all") return false;
  syncAll(true).then(() => sendResponse({ ok: true })).catch((error) => {
    sendResponse({ ok: false, error: error.message });
  });
  return true;
});
