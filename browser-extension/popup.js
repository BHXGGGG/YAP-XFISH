// 通过本机 YAP-XFISH 面板 API 添加 domain_suffix 规则。
// outbound "proxy" 由后端渲染为当前选中节点；同一域名重复添加只会更新出站。
const $ = (id) => document.getElementById(id);
const port = () => Number(localStorage.getItem("port")) || 9677;
const base = () => `http://127.0.0.1:${port()}`;

// 常见二级后缀（example.com.cn / example.co.uk）多取一级
const SECOND_LEVEL = new Set(["com", "net", "org", "gov", "edu", "co", "ac"]);
function rootDomain(host) {
  host = host.replace(/^www\./, "");
  if (/^[\d.]+$/.test(host) || host.includes(":")) return host; // IP 原样
  const parts = host.split(".");
  if (parts.length <= 2) return host;
  const n = SECOND_LEVEL.has(parts[parts.length - 2]) && parts[parts.length - 1].length === 2 ? 3 : 2;
  return parts.slice(-n).join(".");
}

function show(text, cls) {
  const m = $("msg");
  m.textContent = text;
  m.className = cls || "";
}

async function api(method, path, body) {
  const r = await fetch(base() + path, {
    method,
    headers: body ? { "Content-Type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
  return data;
}

async function add(outbound) {
  const domain = $("domain").value.trim().toLowerCase();
  if (!domain) return show("请填写域名", "err");
  show("添加中…");
  try {
    await api("POST", "/api/rules", { name: domain, type: "domain_suffix", payload: domain, outbound });
    show(`已添加：${domain} → ${outbound === "proxy" ? "代理" : "直连"}，刷新网页生效`, "ok");
  } catch (e) {
    show(e instanceof TypeError ? `连不上 YAP-XFISH（${base()}），请确认程序已运行` : e.message, "err");
  }
}

async function init() {
  $("port").value = port();
  $("port").onchange = () => localStorage.setItem("port", $("port").value);
  $("proxy").onclick = () => add("proxy");
  $("direct").onclick = () => add("direct");
  $("open").onclick = () => chrome.tabs.create({ url: base() + "/" });

  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  let host = "";
  try { host = new URL(tab?.url || "").hostname; } catch {}
  $("domain").value = host ? rootDomain(host) : "";
  if (!host) return show("当前页面没有域名，请手动填写", "err");

  // 显示已有规则
  try {
    const rules = await api("GET", "/api/rules");
    const d = $("domain").value;
    const hit = rules.find((r) => r.type === "domain_suffix" && r.payload.toLowerCase() === d);
    if (hit) show(`已有规则：${d} → ${hit.outbound === "direct" ? "直连" : hit.outbound === "block" ? "拦截" : "代理"}`);
  } catch (e) {
    show(e instanceof TypeError ? `连不上 YAP-XFISH（${base()}），请确认程序已运行` : e.message, "err");
  }
}

init();
