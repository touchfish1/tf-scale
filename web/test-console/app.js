const fields = [
  "controlIp",
  "controlUrl",
  "relayUrl",
  "repoPath",
  "keyA",
  "keyB",
  "ipA",
  "ipB",
  "hostA",
  "hostB",
];

const platformNotes = {
  linux:
    "Linux 是当前主测试平台：支持 TUN、UDP transport、P2P direct probe、overlay ping。",
  macos:
    "macOS 有 utun/resolver 设计和部分实现，但 P2P 远程互联仍需要实机验证。",
  windows:
    "Windows 暂时作为测试记录与控制台使用；Wintun、路由和 DNS 客户端仍未实现。",
};

let selectedPlatform = localStorage.getItem("tfscale.platform") || "linux";
let selectedCommand = localStorage.getItem("tfscale.command") || "control";

function $(id) {
  return document.getElementById(id);
}

function readState() {
  const state = {};
  for (const id of fields) {
    state[id] = $(id).value.trim();
  }
  return state;
}

function saveState() {
  for (const id of fields) {
    localStorage.setItem(`tfscale.${id}`, $(id).value);
  }
}

function loadState() {
  for (const id of fields) {
    const value = localStorage.getItem(`tfscale.${id}`);
    if (value !== null) $(id).value = value;
  }
}

function shellQuote(value) {
  if (!value) return "''";
  if (/^[A-Za-z0-9_./:@=-]+$/.test(value)) return value;
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function commands() {
  const s = readState();
  const repo = shellQuote(s.repoPath || "/opt/tf-sacle");
  const controlUrl = shellQuote(s.controlUrl);
  const relayUrl = shellQuote(s.relayUrl);
  const keyA = shellQuote(s.keyA || "<key-a>");
  const keyB = shellQuote(s.keyB || "<key-b>");
  const ipA = s.ipA || "<agent-a-overlay-ip>";
  const ipB = s.ipB || "<agent-b-overlay-ip>";

  return {
    control: `cd ${repo}
export TFSCALE_CONTROL_LISTEN=0.0.0.0:8080
export TFSCALE_CONTROL_URL=${controlUrl}
export TFSCALE_UDP_PROBE_LISTEN=0.0.0.0:3478
export TFSCALE_RELAY_LISTEN=0.0.0.0:9443
export TFSCALE_RELAY_URL=${relayUrl}

scripts/connectivity-relay-check.sh preflight
scripts/connectivity-relay-check.sh build
scripts/connectivity-relay-check.sh control
scripts/connectivity-relay-check.sh relay

key_a="$(scripts/connectivity-relay-check.sh make-key | tail -n 1)"
key_b="$(scripts/connectivity-relay-check.sh make-key | tail -n 1)"
printf 'Agent A key: %s\\nAgent B key: %s\\n' "$key_a" "$key_b"`,

    agentA: `cd ${repo}
export TFSCALE_CONTROL_URL=${controlUrl}
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-a
sudo -E scripts/connectivity-relay-check.sh preflight
sudo -E scripts/connectivity-relay-check.sh agent --login-key ${keyA}
sudo -E scripts/connectivity-relay-check.sh status`,

    agentB: `cd ${repo}
export TFSCALE_CONTROL_URL=${controlUrl}
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-b
sudo -E scripts/connectivity-relay-check.sh preflight
sudo -E scripts/connectivity-relay-check.sh agent --login-key ${keyB}
sudo -E scripts/connectivity-relay-check.sh status`,

    status: `cd ${repo}
target/debug/tfscalectl --control-url ${controlUrl} device list

# Agent A
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-a
sudo -E scripts/connectivity-relay-check.sh status

# Agent B
export TFSCALE_STATE_DIR=/var/lib/tfscale-agent-b
sudo -E scripts/connectivity-relay-check.sh status`,

    ping: `# Agent A -> Agent B
ping -c 3 ${ipB}

# Agent B -> Agent A
ping -c 3 ${ipA}`,

    dns: `# 在 agent 主机执行
target/debug/tfscalectl --control-url ${controlUrl} dns records
sudo -E target/debug/tfscale-agent --state-dir "$TFSCALE_STATE_DIR" dns install
ping -c 3 ${(s.hostB || "<peer-hostname>")}.mesh
sudo -E target/debug/tfscale-agent --state-dir "$TFSCALE_STATE_DIR" dns uninstall`,
  };
}

function renderCommand() {
  $("commandOutput").textContent = commands()[selectedCommand];
  localStorage.setItem("tfscale.command", selectedCommand);
}

function setPlatform(platform) {
  selectedPlatform = platform;
  localStorage.setItem("tfscale.platform", platform);
  document.querySelectorAll(".platform").forEach((button) => {
    button.classList.toggle("active", button.dataset.platform === platform);
  });
  $("platformNote").textContent = platformNotes[platform];
}

function setCommand(command) {
  selectedCommand = command;
  document.querySelectorAll(".tab").forEach((button) => {
    button.classList.toggle("active", button.dataset.command === command);
  });
  renderCommand();
}

function parseStatus() {
  const raw = $("statusInput").value.trim();
  if (!raw) return;
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    $("diagnostics").innerHTML = `<div class="line"><strong>解析失败</strong><span>${error.message}</span></div>`;
    return;
  }

  const peers = parsed.backend?.peers || [];
  const message = parsed.backend?.message || "";
  const direct = peers.filter((peer) => peer.path === "direct").length;
  const relay = peers.filter((peer) => peer.path === "relay").length;
  const unknown = peers.filter((peer) => peer.path === "unknown").length;
  const fast = message.match(/fast_probe_peers=([^ ]+)/)?.[1] || "-";
  const directPaths = message.match(/direct_paths=([^ ]+)/)?.[1] || "-";

  $("diagnostics").innerHTML = [
    ["Device", parsed.device_id || "-"],
    ["Overlay IP", parsed.ipv4 || "-"],
    ["Backend", parsed.backend?.backend_type || "-"],
    ["Healthy", String(parsed.backend?.healthy ?? "-")],
    ["Peers", `direct=${direct} relay=${relay} unknown=${unknown}`],
    ["Fast Probe", fast],
    ["Direct Paths", directPaths],
  ]
    .map(([label, value]) => `<div class="line"><strong>${label}</strong><code>${value}</code></div>`)
    .join("");

  $("overallStatus").textContent =
    direct > 0 ? "P2P direct" : relay > 0 ? "Relay fallback" : "等待打洞";
}

function updateChecklistStatus() {
  const checked = [...document.querySelectorAll("[data-check]")].filter(
    (input) => input.checked,
  ).length;
  $("overallStatus").textContent =
    checked >= 5 ? "验收通过" : checked > 0 ? `进行中 ${checked}/6` : $("overallStatus").textContent;
}

loadState();
setPlatform(selectedPlatform);
setCommand(selectedCommand);
renderCommand();

for (const id of fields) {
  $(id).addEventListener("input", () => {
    saveState();
    renderCommand();
  });
}

document.querySelectorAll(".platform").forEach((button) => {
  button.addEventListener("click", () => setPlatform(button.dataset.platform));
});

document.querySelectorAll(".tab").forEach((button) => {
  button.addEventListener("click", () => setCommand(button.dataset.command));
});

$("copyCommandBtn").addEventListener("click", async () => {
  await navigator.clipboard.writeText($("commandOutput").textContent);
});

$("parseStatusBtn").addEventListener("click", parseStatus);

$("resetBtn").addEventListener("click", () => {
  for (const id of fields) localStorage.removeItem(`tfscale.${id}`);
  location.reload();
});

document.querySelectorAll("[data-check]").forEach((input) => {
  input.addEventListener("change", updateChecklistStatus);
});
