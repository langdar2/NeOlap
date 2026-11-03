// NeOlap web UI – entry module (M0: server status only).
const status = document.getElementById("status");

try {
  const response = await fetch("api/v1/health");
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const health = await response.json();
  status.textContent = `Server läuft – ${health.name} ${health.version}`;
  status.className = "ok";
} catch (err) {
  status.textContent = `Server nicht erreichbar (${err.message})`;
  status.className = "error";
}
